use crate::traits::{StreamEntry, StreamingBackend};
use anyhow::{Context, Result};
use futures_util::future::BoxFuture;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
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
    pub async fn new(redis_url: &str) -> Result<Self> {
        let client = redis::Client::open(redis_url)?;
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
    ) -> BoxFuture<'a, Result<String>> {
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

            cmd.query_async(&mut conn)
                .await
                .context("Failed to add to Redis stream")
        })
    }

    fn stream_read_latest<'a>(
        &'a self,
        stream_key: &'a str,
        count: usize,
    ) -> BoxFuture<'a, Result<Vec<StreamEntry>>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            // XREVRANGE is more appropriate for "latest" N entries.
            // XREAD with "0" reads from the beginning, not necessarily the latest N.
            // Let's use XREVRANGE for this method.
            let raw_result: redis::Value = redis::cmd("XREVRANGE")
                .arg(stream_key)
                .arg("+") // Start from the latest ID
                .arg("-") // End at the earliest ID
                .arg("COUNT")
                .arg(count)
                .query_async(&mut conn)
                .await
                .context("Failed to read latest from Redis stream using XREVRANGE")?;

            let mut entries = Vec::new();
            if let redis::Value::Array(redis_entries) = raw_result {
                for entry_val in redis_entries {
                    if let redis::Value::Array(entry_parts) = entry_val {
                        if entry_parts.len() >= 2 {
                            if let Some(redis::Value::BulkString(id_bytes)) = entry_parts.first() {
                                let id = String::from_utf8_lossy(id_bytes).to_string();
                                if let Some(redis::Value::Array(field_values)) = entry_parts.get(1)
                                {
                                    let mut fields = Vec::new();
                                    for chunk in field_values.chunks(2) {
                                        if chunk.len() == 2 {
                                            if let (
                                                Some(redis::Value::BulkString(f_bytes)),
                                                Some(redis::Value::BulkString(v_bytes)),
                                            ) = (chunk.first(), chunk.get(1))
                                            {
                                                fields.push((
                                                    String::from_utf8_lossy(f_bytes).to_string(),
                                                    String::from_utf8_lossy(v_bytes).to_string(),
                                                ));
                                            }
                                        }
                                    }
                                    entries.push((id, fields));
                                }
                            }
                        }
                    }
                }
            }
            debug!(
                "[Stream] Read {} latest entries from '{}'",
                entries.len(),
                stream_key
            );
            Ok(entries)
        })
    }

    fn stream_read<'a>(
        &'a self,
        stream_key: &'a str,
        last_id: &'a str,
        count: usize,
        block_ms: Option<usize>,
    ) -> BoxFuture<'a, Result<Vec<StreamEntry>>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let mut options = redis::streams::StreamReadOptions::default().count(count);
            if let Some(ms) = block_ms {
                options = options.block(ms);
            }

            let raw_result: redis::Value = conn
                .xread_options(&[stream_key], &[last_id], &options)
                .await
                .context("Failed to read from Redis stream using XREAD")?;

            let mut all_entries = Vec::new();

            if let redis::Value::Array(streams) = raw_result {
                for stream in streams {
                    if let redis::Value::Array(stream_parts) = stream {
                        if stream_parts.len() >= 2 {
                            if let Some(redis::Value::Array(entries)) = stream_parts.get(1) {
                                for entry in entries {
                                    if let redis::Value::Array(entry_parts) = entry {
                                        if entry_parts.len() >= 2 {
                                            if let Some(redis::Value::BulkString(id_bytes)) =
                                                entry_parts.first()
                                            {
                                                let id =
                                                    String::from_utf8_lossy(id_bytes).to_string();
                                                if let Some(redis::Value::Array(field_values)) =
                                                    entry_parts.get(1)
                                                {
                                                    let mut fields = Vec::new();
                                                    for chunk in field_values.chunks(2) {
                                                        if chunk.len() == 2 {
                                                            if let (
                                                                Some(redis::Value::BulkString(
                                                                    f_bytes,
                                                                )),
                                                                Some(redis::Value::BulkString(
                                                                    v_bytes,
                                                                )),
                                                            ) = (chunk.first(), chunk.get(1))
                                                            {
                                                                fields.push((
                                                                    String::from_utf8_lossy(
                                                                        f_bytes,
                                                                    )
                                                                    .to_string(),
                                                                    String::from_utf8_lossy(
                                                                        v_bytes,
                                                                    )
                                                                    .to_string(),
                                                                ));
                                                            }
                                                        }
                                                    }
                                                    all_entries.push((id, fields));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
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
}
