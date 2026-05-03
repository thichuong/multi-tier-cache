//! Cache invalidation and synchronization module
//!
//! This module provides cross-instance cache invalidation using Redis Pub/Sub.
//! It supports both cache removal (invalidation) and cache updates (refresh).

use crate::error::CacheResult;
use crate::traits::StreamingBackend;
use bytes::Bytes;
use futures_util::StreamExt;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Invalidation message types sent across cache instances via Redis Pub/Sub
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InvalidationMessage {
    /// Remove a single key from all cache instances
    Remove { key: String },

    /// Update a key with new value across all cache instances
    /// This is more efficient than Remove for hot keys as it avoids cache miss
    Update {
        key: String,
        #[serde(with = "serde_bytes_wrapper")]
        value: Bytes,
        #[serde(skip_serializing_if = "Option::is_none")]
        ttl_secs: Option<u64>,
    },

    /// Remove all keys matching a pattern from all cache instances
    /// Uses glob-style patterns (e.g., "user:*", "product:123:*")
    RemovePattern { pattern: String },

    /// Bulk remove multiple keys at once
    RemoveBulk { keys: Vec<String> },
}

impl InvalidationMessage {
    /// Create a Remove message
    pub fn remove(key: impl Into<String>) -> Self {
        Self::Remove { key: key.into() }
    }

    /// Create an Update message
    pub fn update(key: impl Into<String>, value: Bytes, ttl: Option<Duration>) -> Self {
        Self::Update {
            key: key.into(),
            value,
            ttl_secs: ttl.map(|d| d.as_secs()),
        }
    }

    /// Create a `RemovePattern` message
    pub fn remove_pattern(pattern: impl Into<String>) -> Self {
        Self::RemovePattern {
            pattern: pattern.into(),
        }
    }

    /// Create a `RemoveBulk` message
    #[must_use]
    pub fn remove_bulk(keys: Vec<String>) -> Self {
        Self::RemoveBulk { keys }
    }

    /// Serialize to JSON for transmission
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_json(&self) -> CacheResult<String> {
        serde_json::to_string(self).map_err(|e| {
            crate::error::CacheError::SerializationError(format!(
                "Failed to serialize invalidation message: {e}"
            ))
        })
    }

    /// Deserialize from JSON
    ///
    /// # Errors
    ///
    /// Returns an error if deserialization fails.
    pub fn from_json(json: &str) -> CacheResult<Self> {
        serde_json::from_str(json).map_err(|e| {
            crate::error::CacheError::SerializationError(format!(
                "Failed to deserialize invalidation message: {e}"
            ))
        })
    }

    /// Get TTL as Duration if present
    pub fn ttl(&self) -> Option<Duration> {
        match self {
            Self::Update { ttl_secs, .. } => ttl_secs.map(Duration::from_secs),
            _ => None,
        }
    }
}

/// Helper module for Bytes serialization in JSON
mod serde_bytes_wrapper {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Bytes, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // For JSON, we use a vector of bytes.
        // In a real production system, we'd use Base64.
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
    where
        D: Deserializer<'de>,
    {
        let v: Vec<u8> = Vec::deserialize(deserializer)?;
        Ok(Bytes::from(v))
    }
}

/// Configuration for cache invalidation
#[derive(Debug, Clone)]
pub struct InvalidationConfig {
    /// Redis Pub/Sub channel name for invalidation messages
    pub channel: String,

    /// Whether to automatically broadcast invalidation on writes
    pub auto_broadcast_on_write: bool,

    /// Whether to also publish invalidation events to Redis Streams for audit
    pub enable_audit_stream: bool,

    /// Redis Stream name for invalidation audit trail
    pub audit_stream: String,

    /// Maximum length of audit stream (older entries are trimmed)
    pub audit_stream_maxlen: Option<usize>,
}

impl Default for InvalidationConfig {
    fn default() -> Self {
        Self {
            channel: "cache:invalidate".to_string(),
            auto_broadcast_on_write: false, // Conservative default
            enable_audit_stream: false,
            audit_stream: "cache:invalidations".to_string(),
            audit_stream_maxlen: Some(10000),
        }
    }
}

/// Handle for sending invalidation messages
pub struct InvalidationPublisher {
    connection: redis::aio::ConnectionManager,
    config: InvalidationConfig,
}

impl InvalidationPublisher {
    /// Create a new publisher
    #[must_use]
    pub fn new(connection: redis::aio::ConnectionManager, config: InvalidationConfig) -> Self {
        Self { connection, config }
    }

    /// Publish an invalidation message to all subscribers
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or publishing fails.
    pub async fn publish(&mut self, message: &InvalidationMessage) -> CacheResult<()> {
        let json = message.to_json()?;

        // Publish to Pub/Sub channel
        let _: () = self
            .connection
            .publish(&self.config.channel, &json)
            .await
            .map_err(|e| {
                crate::error::CacheError::InvalidationError(format!(
                    "Failed to publish invalidation message: {e}"
                ))
            })?;

        // Optionally publish to audit stream
        if self.config.enable_audit_stream
            && let Err(e) = self.publish_to_audit_stream(message).await
        {
            // Don't fail the invalidation if audit logging fails
            warn!("Failed to publish to audit stream: {}", e);
        }

        Ok(())
    }

    /// Publish to audit stream for observability
    async fn publish_to_audit_stream(&mut self, message: &InvalidationMessage) -> CacheResult<()> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs()
            .to_string();

        // Use &str to avoid unnecessary allocations
        let (type_str, key_str): (&str, &str);
        let extra_str: String;

        match message {
            InvalidationMessage::Remove { key } => {
                type_str = "remove";
                key_str = key.as_str();
                extra_str = String::new();
            }
            InvalidationMessage::Update { key, .. } => {
                type_str = "update";
                key_str = key.as_str();
                extra_str = String::new();
            }
            InvalidationMessage::RemovePattern { pattern } => {
                type_str = "remove_pattern";
                key_str = pattern.as_str();
                extra_str = String::new();
            }
            InvalidationMessage::RemoveBulk { keys } => {
                type_str = "remove_bulk";
                key_str = "";
                extra_str = keys.len().to_string();
            }
        }

        let mut fields = vec![("type", type_str), ("timestamp", timestamp.as_str())];

        if !key_str.is_empty() {
            fields.push(("key", key_str));
        }
        if !extra_str.is_empty() {
            fields.push(("count", extra_str.as_str()));
        }

        let mut cmd = redis::cmd("XADD");
        cmd.arg(&self.config.audit_stream);

        if let Some(maxlen) = self.config.audit_stream_maxlen {
            cmd.arg("MAXLEN").arg("~").arg(maxlen);
        }

        cmd.arg("*"); // Auto-generate ID

        for (key, value) in fields {
            cmd.arg(key).arg(value);
        }

        let _: String = cmd.query_async(&mut self.connection).await.map_err(|e| {
            crate::error::CacheError::BackendError(format!("Failed to add to audit stream: {e}"))
        })?;

        Ok(())
    }
}

/// Statistics for invalidation operations
#[derive(Debug, Default, Clone)]
pub struct InvalidationStats {
    /// Number of invalidation messages published
    pub messages_sent: u64,

    /// Number of invalidation messages received
    pub messages_received: u64,

    /// Number of Remove operations performed
    pub removes_received: u64,

    /// Number of Update operations performed
    pub updates_received: u64,

    /// Number of `RemovePattern` operations performed
    pub patterns_received: u64,

    /// Number of `RemoveBulk` operations performed
    pub bulk_removes_received: u64,

    /// Number of failed message processing attempts
    pub processing_errors: u64,
}

use std::sync::atomic::{AtomicU64, Ordering};

/// Thread-safe statistics for invalidation operations
#[derive(Debug, Default)]
pub struct AtomicInvalidationStats {
    pub messages_sent: AtomicU64,
    pub messages_received: AtomicU64,
    pub removes_received: AtomicU64,
    pub updates_received: AtomicU64,
    pub patterns_received: AtomicU64,
    pub bulk_removes_received: AtomicU64,
    pub processing_errors: AtomicU64,
}

impl AtomicInvalidationStats {
    pub fn snapshot(&self) -> InvalidationStats {
        InvalidationStats {
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            messages_received: self.messages_received.load(Ordering::Relaxed),
            removes_received: self.removes_received.load(Ordering::Relaxed),
            updates_received: self.updates_received.load(Ordering::Relaxed),
            patterns_received: self.patterns_received.load(Ordering::Relaxed),
            bulk_removes_received: self.bulk_removes_received.load(Ordering::Relaxed),
            processing_errors: self.processing_errors.load(Ordering::Relaxed),
        }
    }
}

use std::sync::Arc;

/// Handle for subscribing to invalidation messages
///
/// This spawns a background task that listens to Redis Pub/Sub and processes
/// invalidation messages by calling the provided handler callback.
pub struct InvalidationSubscriber {
    /// Redis client for creating Pub/Sub connections
    client: redis::Client,
    /// Configuration
    config: InvalidationConfig,
    /// Statistics
    stats: Arc<AtomicInvalidationStats>,
    /// Shutdown signal sender
    shutdown_tx: broadcast::Sender<()>,
}

impl InvalidationSubscriber {
    /// Create a new subscriber
    ///
    /// # Arguments
    /// * `redis_url` - Redis connection URL
    /// * `config` - Invalidation configuration
    /// # Errors
    ///
    /// Returns an error if Redis client creation fails.
    pub fn new(redis_url: &str, config: InvalidationConfig) -> CacheResult<Self> {
        let client = redis::Client::open(redis_url).map_err(|e| {
            crate::error::CacheError::ConfigError(format!(
                "Failed to create Redis client for subscriber: {e}"
            ))
        })?;

        let (shutdown_tx, _) = broadcast::channel(1);

        Ok(Self {
            client,
            config,
            stats: Arc::new(AtomicInvalidationStats::default()),
            shutdown_tx,
        })
    }

    /// Get a snapshot of current statistics
    #[must_use]
    pub fn stats(&self) -> InvalidationStats {
        self.stats.snapshot()
    }

    /// Start the subscriber background task
    ///
    /// # Arguments
    /// * `handler` - Async function to handle each invalidation message
    ///
    /// # Returns
    /// Join handle for the background task
    pub fn start<F, Fut>(&self, handler: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn(InvalidationMessage) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = CacheResult<()>> + Send + 'static,
    {
        let client = self.client.clone();
        let channel = self.config.channel.clone();
        let stats = Arc::clone(&self.stats);
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        tokio::spawn(async move {
            let handler = Arc::new(handler);

            loop {
                // Check for shutdown signal
                if shutdown_rx.try_recv().is_ok() {
                    info!("Invalidation subscriber shutting down...");
                    break;
                }

                // Attempt to connect and subscribe
                match Self::run_subscriber_loop(
                    &client,
                    &channel,
                    Arc::clone(&handler),
                    Arc::clone(&stats),
                    &mut shutdown_rx,
                )
                .await
                {
                    Ok(()) => {
                        info!("Invalidation subscriber loop completed normally");
                        break;
                    }
                    Err(e) => {
                        error!(
                            "Invalidation subscriber error: {}. Reconnecting in 5s...",
                            e
                        );
                        stats.processing_errors.fetch_add(1, Ordering::Relaxed);

                        // Wait before reconnecting
                        tokio::select! {
                            () = tokio::time::sleep(Duration::from_secs(5)) => {},
                            _ = shutdown_rx.recv() => {
                                info!("Invalidation subscriber shutting down...");
                                break;
                            }
                        }
                    }
                }
            }
        })
    }

    /// Internal subscriber loop
    async fn run_subscriber_loop<F, Fut>(
        client: &redis::Client,
        channel: &str,
        handler: Arc<F>,
        stats: Arc<AtomicInvalidationStats>,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> CacheResult<()>
    where
        F: Fn(InvalidationMessage) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = CacheResult<()>> + Send + 'static,
    {
        let mut pubsub = client.get_async_pubsub().await.map_err(|e| {
            crate::error::CacheError::BackendError(format!("Failed to get pubsub connection: {e}"))
        })?;

        // Subscribe to channel
        pubsub.subscribe(channel).await.map_err(|e| {
            crate::error::CacheError::InvalidationError(format!(
                "Failed to subscribe to channel: {e}"
            ))
        })?;

        info!("Subscribed to invalidation channel: {}", channel);

        // Get message stream
        let mut stream = pubsub.on_message();

        loop {
            // Wait for message or shutdown signal
            tokio::select! {
                msg_result = stream.next() => {
                    match msg_result {
                        Some(msg) => {
                            // Get payload
                            let payload: String = match msg.get_payload() {
                                Ok(p) => p,
                                Err(e) => {
                                    warn!("Failed to get message payload: {}", e);
                                    stats.processing_errors.fetch_add(1, Ordering::Relaxed);
                                    continue;
                                }
                            };

                            // Deserialize message
                            let invalidation_msg = match InvalidationMessage::from_json(&payload) {
                                Ok(m) => m,
                                Err(e) => {
                                    warn!("Failed to deserialize invalidation message: {}", e);
                                    stats.processing_errors.fetch_add(1, Ordering::Relaxed);
                                    continue;
                                }
                            };

                            // Update stats
                            stats.messages_received.fetch_add(1, Ordering::Relaxed);
                            match &invalidation_msg {
                                InvalidationMessage::Remove { .. } => {
                                    stats.removes_received.fetch_add(1, Ordering::Relaxed);
                                }
                                InvalidationMessage::Update { .. } => {
                                    stats.updates_received.fetch_add(1, Ordering::Relaxed);
                                }
                                InvalidationMessage::RemovePattern { .. } => {
                                    stats.patterns_received.fetch_add(1, Ordering::Relaxed);
                                }
                                InvalidationMessage::RemoveBulk { .. } => {
                                    stats.bulk_removes_received.fetch_add(1, Ordering::Relaxed);
                                }
                            }

                            // Call handler
                            if let Err(e) = handler(invalidation_msg).await {
                                error!("Invalidation handler error: {}", e);
                                stats.processing_errors.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        None => {
                            // Stream ended
                            return Err(crate::error::CacheError::InvalidationError("Pub/Sub message stream ended".to_string()));
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    return Ok(());
                }
            }
        }
    }

    /// Signal the subscriber to shutdown
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
    }
}

/// Reliable subscriber using Redis Streams and Consumer Groups
pub struct ReliableStreamSubscriber {
    client: redis::Client,
    config: InvalidationConfig,
    stats: Arc<AtomicInvalidationStats>,
    shutdown_tx: broadcast::Sender<()>,
    group_name: String,
    consumer_name: String,
}

impl ReliableStreamSubscriber {
    /// Create a new `ReliableStreamSubscriber`
    ///
    /// # Errors
    ///
    /// Returns an error if the Redis client fails to open.
    pub fn new(redis_url: &str, config: InvalidationConfig, group_name: &str) -> CacheResult<Self> {
        let client = redis::Client::open(redis_url).map_err(|e| {
            crate::error::CacheError::ConfigError(format!(
                "Failed to create Redis client for reliable subscriber: {e}"
            ))
        })?;

        let (shutdown_tx, _) = broadcast::channel(1);
        let consumer_name = format!("consumer-{}", Uuid::new_v4());

        Ok(Self {
            client,
            config,
            stats: Arc::new(AtomicInvalidationStats::default()),
            shutdown_tx,
            group_name: group_name.to_string(),
            consumer_name,
        })
    }

    pub fn start<F, Fut>(&self, handler: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn(InvalidationMessage) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = CacheResult<()>> + Send + 'static,
    {
        let client = self.client.clone();
        let stream_key = self.config.channel.clone();
        let group_name = self.group_name.clone();
        let consumer_name = self.consumer_name.clone();
        let handler = Arc::new(handler);
        let stats = self.stats.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        tokio::spawn(async move {
            info!(
                stream = %stream_key,
                group = %group_name,
                consumer = %consumer_name,
                "Starting reliable stream subscriber"
            );

            // 1. Ensure stream and group exist
            let redis_backend = crate::redis_streams::RedisStreams::new(
                client.get_connection_info().addr().to_string().as_str(),
            )
            .await;
            if let Ok(backend) = redis_backend {
                let _ = backend
                    .stream_create_group(&stream_key, &group_name, "0")
                    .await;

                loop {
                    // Check shutdown before starting loop
                    if shutdown_rx.try_recv().is_ok() {
                        break;
                    }

                    if let Err(e) = Self::run_reliable_loop(
                        &backend,
                        &stream_key,
                        &group_name,
                        &consumer_name,
                        handler.clone(),
                        stats.clone(),
                        &mut shutdown_rx,
                    )
                    .await
                    {
                        error!("Reliable subscriber loop error: {}", e);

                        tokio::select! {
                            () = tokio::time::sleep(Duration::from_secs(5)) => {},
                            _ = shutdown_rx.recv() => break,
                        }
                    } else {
                        break; // Normal shutdown
                    }
                }
            }
        })
    }

    async fn run_reliable_loop<F, Fut>(
        backend: &dyn crate::traits::StreamingBackend,
        stream_key: &str,
        group_name: &str,
        consumer_name: &str,
        handler: Arc<F>,
        stats: Arc<AtomicInvalidationStats>,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> CacheResult<()>
    where
        F: Fn(InvalidationMessage) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = CacheResult<()>> + Send + 'static,
    {
        loop {
            tokio::select! {
                entries_result = backend.stream_read_group(stream_key, group_name, consumer_name, 10, Some(5000)) => {
                    let entries = entries_result?;
                    if entries.is_empty() { continue; }

                    let mut processed_ids = Vec::new();
                    for (id, fields) in entries {
                        // Find "payload" field or use first field if it looks like JSON
                        let payload = fields.iter().find(|(k, _)| k == "payload")
                            .map(|(_, v)| v.as_str())
                            .or_else(|| fields.first().map(|(_, v)| v.as_str()));

                        if let Some(msg) = payload.and_then(|json| InvalidationMessage::from_json(json).ok()) {
                            stats.messages_received.fetch_add(1, Ordering::Relaxed);
                            if let Err(e) = handler(msg).await {
                                error!("Reliable handler error: {}", e);
                                stats.processing_errors.fetch_add(1, Ordering::Relaxed);
                            } else {
                                processed_ids.push(id);
                            }
                        }
                    }

                    if !processed_ids.is_empty() {
                        backend.stream_ack(stream_key, group_name, &processed_ids).await?;
                    }
                }
                _ = shutdown_rx.recv() => return Ok(()),
            }
        }
    }

    /// Signal the subscriber to shutdown
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(()).unwrap_or(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalidation_message_serialization() -> CacheResult<()> {
        // Test Remove
        let msg = InvalidationMessage::remove("test_key");
        let json = msg.to_json()?;
        let parsed = InvalidationMessage::from_json(&json)?;
        match parsed {
            InvalidationMessage::Remove { key } => assert_eq!(key, "test_key"),
            _ => panic!("Wrong message type"),
        }

        // Test Update
        let msg = InvalidationMessage::update(
            "test_key",
            Bytes::from("{\"value\": 123}"),
            Some(Duration::from_secs(3600)),
        );

        if let InvalidationMessage::Update {
            key,
            value,
            ttl_secs,
        } = msg
        {
            assert_eq!(key, "test_key");
            assert_eq!(value, Bytes::from("{\"value\": 123}"));
            assert_eq!(ttl_secs, Some(3600));
        } else {
            panic!("Expected Update message");
        }

        // Test RemovePattern
        let msg = InvalidationMessage::remove_pattern("user:*");
        let json = msg.to_json()?;
        let parsed = InvalidationMessage::from_json(&json)?;
        match parsed {
            InvalidationMessage::RemovePattern { pattern } => assert_eq!(pattern, "user:*"),
            _ => panic!("Wrong message type"),
        }

        // Test RemoveBulk
        let msg = InvalidationMessage::remove_bulk(vec!["key1".to_string(), "key2".to_string()]);
        let json = msg.to_json()?;
        let parsed = InvalidationMessage::from_json(&json)?;
        match parsed {
            InvalidationMessage::RemoveBulk { keys } => assert_eq!(keys, vec!["key1", "key2"]),
            _ => panic!("Wrong message type"),
        }
        Ok(())
    }

    #[test]
    fn test_invalidation_config_default() {
        let config = InvalidationConfig::default();
        assert_eq!(config.channel, "cache:invalidate");
        assert!(!config.auto_broadcast_on_write);
        assert!(!config.enable_audit_stream);
    }
}
