use crate::error::CacheResult;
use crate::traits::{StreamEntry, StreamingBackend};
use futures_util::future::BoxFuture;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use tracing::debug;

/// Redis Streams client for event-driven architectures
#[derive(Clone)]
pub struct RedisStreams {
    conn_manager: ConnectionManager,
}

impl RedisStreams {
    /// Create a new `RedisStreams` instance
    ///
    /// # Errors
    ///
    /// Returns an error if the Redis connection fails.
    pub async fn new(redis_url: &str) -> CacheResult<Self> {
        let client = redis::Client::open(redis_url).map_err(|e| {
            crate::error::CacheError::ConfigError(format!(
                "Failed to create Redis client for streams: {e}"
            ))
        })?;
        let conn_manager = ConnectionManager::new(client).await?;

        debug!("Redis Streams initialized at {}", redis_url);

        Ok(Self { conn_manager })
    }
}

/// Implement `StreamingBackend` trait for `RedisStreams`
impl StreamingBackend for RedisStreams {
    fn stream_add<'a>(
        &'a self,
        stream_key: &'a str,
        fields: Vec<(String, String)>,
        maxlen: Option<usize>,
    ) -> BoxFuture<'a, CacheResult<String>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let mut cmd = redis::cmd("XADD");
            cmd.arg(stream_key);

            if let Some(max) = maxlen {
                cmd.arg("MAXLEN").arg("~").arg(max);
            }

            cmd.arg("*");

            for (field, value) in fields {
                cmd.arg(field).arg(value);
            }

            cmd.query_async(&mut conn).await.map_err(|e| {
                crate::error::CacheError::BackendError(format!(
                    "Failed to add to Redis stream: {e}"
                ))
            })
        })
    }

    fn stream_read_latest<'a>(
        &'a self,
        stream_key: &'a str,
        count: usize,
    ) -> BoxFuture<'a, CacheResult<Vec<StreamEntry>>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            // XREVRANGE returns Vec<(String, Vec<(String, String)>)> (id, fields)
            let raw_entries: Vec<(String, Vec<(String, String)>)> = redis::cmd("XREVRANGE")
                .arg(stream_key)
                .arg("+")
                .arg("-")
                .arg("COUNT")
                .arg(count)
                .query_async(&mut conn)
                .await
                .map_err(|e| {
                    crate::error::CacheError::BackendError(format!(
                        "Failed to read latest from Redis stream using XREVRANGE: {e}"
                    ))
                })?;

            debug!(
                "[Stream] Read {} latest entries from '{}'",
                raw_entries.len(),
                stream_key
            );
            Ok(raw_entries)
        })
    }

    fn stream_read<'a>(
        &'a self,
        stream_key: &'a str,
        last_id: &'a str,
        count: usize,
        block_ms: Option<usize>,
    ) -> BoxFuture<'a, CacheResult<Vec<StreamEntry>>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let mut options = redis::streams::StreamReadOptions::default().count(count);
            if let Some(ms) = block_ms {
                options = options.block(ms);
            }

            // XREAD returns Vec<(String, Vec<(String, Vec<(String, String)>)>)> -> (stream_name, entries)
            let result: Vec<(String, Vec<(String, Vec<(String, String)>)>)> = conn
                .xread_options(&[stream_key], &[last_id], &options)
                .await
                .map_err(|e| {
                    crate::error::CacheError::BackendError(format!(
                        "Failed to read from Redis stream using XREAD: {e}"
                    ))
                })?;

            let mut all_entries = Vec::new();
            for (_stream, entries) in result {
                for (id, fields) in entries {
                    all_entries.push((id, fields));
                }
            }

            debug!(
                "[Stream] XREAD retrieved {} entries from '{}'",
                all_entries.len(),
                stream_key
            );
            Ok(all_entries)
        })
    }

    fn stream_create_group<'a>(
        &'a self,
        stream_key: &'a str,
        group_name: &'a str,
        id: &'a str,
    ) -> BoxFuture<'a, CacheResult<()>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let _: () = redis::cmd("XGROUP")
                .arg("CREATE")
                .arg(stream_key)
                .arg(group_name)
                .arg(id)
                .arg("MKSTREAM")
                .query_async(&mut conn)
                .await
                .map_err(|e| {
                    crate::error::CacheError::BackendError(format!(
                        "Failed to create Redis stream consumer group: {e}"
                    ))
                })?;
            Ok(())
        })
    }

    fn stream_read_group<'a>(
        &'a self,
        stream_key: &'a str,
        group_name: &'a str,
        consumer_name: &'a str,
        count: usize,
        block_ms: Option<usize>,
    ) -> BoxFuture<'a, CacheResult<Vec<StreamEntry>>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let mut options = redis::streams::StreamReadOptions::default()
                .group(group_name, consumer_name)
                .count(count);

            if let Some(ms) = block_ms {
                options = options.block(ms);
            }

            // XREAD GROUP returns same structure as XREAD
            let result: Vec<(String, Vec<(String, Vec<(String, String)>)>)> = conn
                .xread_options(&[stream_key], &[">"], &options)
                .await
                .map_err(|e| {
                    crate::error::CacheError::BackendError(format!(
                        "Failed to read from Redis stream group: {e}"
                    ))
                })?;

            let mut all_entries = Vec::new();
            for (_stream, entries) in result {
                for (id, fields) in entries {
                    all_entries.push((id, fields));
                }
            }
            Ok(all_entries)
        })
    }

    fn stream_ack<'a>(
        &'a self,
        stream_key: &'a str,
        group_name: &'a str,
        ids: &'a [String],
    ) -> BoxFuture<'a, CacheResult<()>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let _: usize = conn.xack(stream_key, group_name, ids).await.map_err(|e| {
                crate::error::CacheError::BackendError(format!("Failed to ACK stream entries: {e}"))
            })?;
            Ok(())
        })
    }
}
