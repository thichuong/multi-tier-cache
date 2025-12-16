//! Postcard Codec using `postcard`
//!
//! This codec uses postcard for compact binary serialization with serde compatibility.

use crate::traits::CacheCodec;
use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Postcard Codec using `postcard`
///
/// This codec provides compact, `#![no_std]` friendly binary serialization using postcard.
/// Postcard is designed for embedded and constrained systems, offering excellent performance
/// and small binary sizes.
///
/// # Performance
///
/// Postcard excels in scenarios where:
/// - Compact binary representation is important
/// - Deterministic serialization is needed
/// - Cross-platform compatibility is important
///
/// # Format
///
/// Postcard uses a simple, efficient binary format that is:
/// - Deterministic (same input always produces same output)
/// - Self-describing (includes type information)
/// - Compact (minimal overhead)
#[cfg_attr(docsrs, doc(cfg(feature = "postcard")))]
#[derive(Debug, Default, Clone)]
pub struct PostcardCodec;

impl CacheCodec for PostcardCodec {
    fn serialize<T: Serialize + ?Sized>(&self, value: &T) -> Result<Vec<u8>> {
        Ok(postcard::to_allocvec(value)?)
    }

    fn deserialize<T: DeserializeOwned>(&self, bytes: &[u8]) -> Result<T> {
        Ok(postcard::from_bytes(bytes)?)
    }

    fn name(&self) -> &'static str {
        "postcard"
    }
}
