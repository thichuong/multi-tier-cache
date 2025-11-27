//! Cache invalidation and synchronization module
//!
//! This module provides cross-instance cache invalidation using Redis Pub/Sub.
//! It supports both cache removal (invalidation) and cache updates (refresh).

use anyhow::{Context, Result};
use futures_util::StreamExt;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};

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
        value: serde_json::Value,
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
    pub fn update(key: impl Into<String>, value: serde_json::Value, ttl: Option<Duration>) -> Self {
        Self::Update {
            key: key.into(),
            value,
            ttl_secs: ttl.map(|d| d.as_secs()),
        }
    }

    /// Create a RemovePattern message
    pub fn remove_pattern(pattern: impl Into<String>) -> Self {
        Self::RemovePattern {
            pattern: pattern.into(),
        }
    }

    /// Create a RemoveBulk message
    pub fn remove_bulk(keys: Vec<String>) -> Self {
        Self::RemoveBulk { keys }
    }

    /// Serialize to JSON for transmission
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).context("Failed to serialize invalidation message")
    }

    /// Deserialize from JSON
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("Failed to deserialize invalidation message")
    }

    /// Get TTL as Duration if present
    pub fn ttl(&self) -> Option<Duration> {
        match self {
            Self::Update { ttl_secs, .. } => ttl_secs.map(Duration::from_secs),
            _ => None,
        }
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
    pub fn new(connection: redis::aio::ConnectionManager, config: InvalidationConfig) -> Self {
        Self { connection, config }
    }

    /// Publish an invalidation message to all subscribers
    pub async fn publish(&mut self, message: &InvalidationMessage) -> Result<()> {
        let json = message.to_json()?;

        // Publish to Pub/Sub channel
        let _: () = self
            .connection
            .publish(&self.config.channel, &json)
            .await
            .context("Failed to publish invalidation message")?;

        // Optionally publish to audit stream
        if self.config.enable_audit_stream {
            if let Err(e) = self.publish_to_audit_stream(message).await {
                // Don't fail the invalidation if audit logging fails
                warn!("Failed to publish to audit stream: {}", e);
            }
        }

        Ok(())
    }

    /// Publish to audit stream for observability
    async fn publish_to_audit_stream(&mut self, message: &InvalidationMessage) -> Result<()> {
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

        let _: String = cmd
            .query_async(&mut self.connection)
            .await
            .context("Failed to add to audit stream")?;

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

    /// Number of RemovePattern operations performed
    pub patterns_received: u64,

    /// Number of RemoveBulk operations performed
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
use tokio::sync::broadcast;

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
    pub fn new(redis_url: &str, config: InvalidationConfig) -> Result<Self> {
        let client = redis::Client::open(redis_url)
            .context("Failed to create Redis client for subscriber")?;

        let (shutdown_tx, _) = broadcast::channel(1);

        Ok(Self {
            client,
            config,
            stats: Arc::new(AtomicInvalidationStats::default()),
            shutdown_tx,
        })
    }

    /// Get a snapshot of current statistics
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
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
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
                    Ok(_) => {
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
                            _ = tokio::time::sleep(Duration::from_secs(5)) => {},
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
    ) -> Result<()>
    where
        F: Fn(InvalidationMessage) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        // Get Pub/Sub connection
        let mut pubsub = client
            .get_async_pubsub()
            .await
            .context("Failed to get pubsub connection")?;

        // Subscribe to channel
        pubsub
            .subscribe(channel)
            .await
            .context("Failed to subscribe to channel")?;

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
                            return Err(anyhow::anyhow!("Pub/Sub message stream ended"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalidation_message_serialization() {
        // Test Remove
        let msg = InvalidationMessage::remove("test_key");
        let json = msg.to_json().unwrap();
        let parsed = InvalidationMessage::from_json(&json).unwrap();
        match parsed {
            InvalidationMessage::Remove { key } => assert_eq!(key, "test_key"),
            _ => panic!("Wrong message type"),
        }

        // Test Update
        let msg = InvalidationMessage::update(
            "test_key",
            serde_json::json!({"value": 123}),
            Some(Duration::from_secs(300)),
        );
        let json = msg.to_json().unwrap();
        let parsed = InvalidationMessage::from_json(&json).unwrap();
        match parsed {
            InvalidationMessage::Update {
                key,
                value,
                ttl_secs,
            } => {
                assert_eq!(key, "test_key");
                assert_eq!(value, serde_json::json!({"value": 123}));
                assert_eq!(ttl_secs, Some(300));
            }
            _ => panic!("Wrong message type"),
        }

        // Test RemovePattern
        let msg = InvalidationMessage::remove_pattern("user:*");
        let json = msg.to_json().unwrap();
        let parsed = InvalidationMessage::from_json(&json).unwrap();
        match parsed {
            InvalidationMessage::RemovePattern { pattern } => assert_eq!(pattern, "user:*"),
            _ => panic!("Wrong message type"),
        }

        // Test RemoveBulk
        let msg = InvalidationMessage::remove_bulk(vec!["key1".to_string(), "key2".to_string()]);
        let json = msg.to_json().unwrap();
        let parsed = InvalidationMessage::from_json(&json).unwrap();
        match parsed {
            InvalidationMessage::RemoveBulk { keys } => assert_eq!(keys, vec!["key1", "key2"]),
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_invalidation_config_default() {
        let config = InvalidationConfig::default();
        assert_eq!(config.channel, "cache:invalidate");
        assert_eq!(config.auto_broadcast_on_write, false);
        assert_eq!(config.enable_audit_stream, false);
    }
}
