//! SIMD JSON Codec using `simd-json`

use crate::traits::CacheCodec;
use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// JSON Codec using `simd-json`
///
/// This codec provides faster JSON serialization/deserialization using SIMD instructions.
#[cfg_attr(docsrs, doc(cfg(feature = "simd_json")))]
#[derive(Debug, Default, Clone)]
pub struct SimdJsonCodec;

impl CacheCodec for SimdJsonCodec {
    fn serialize<T: Serialize + ?Sized>(&self, value: &T) -> Result<Vec<u8>> {
        Ok(simd_json::to_vec(value)?)
    }

    fn deserialize<T: DeserializeOwned>(&self, bytes: &[u8]) -> Result<T> {
        // simd-json requires mutable bytes for in-place modification
        // We'll have to copy since our interface takes immutable slice
        let mut bytes_copy = bytes.to_vec();
        Ok(simd_json::from_slice(&mut bytes_copy)?)
    }

    fn name(&self) -> &'static str {
        "simd_json"
    }
}
