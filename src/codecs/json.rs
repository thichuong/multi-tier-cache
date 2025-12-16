//! JSON Codec using `serde_json`

use crate::traits::CacheCodec;
use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// JSON Codec using `serde_json`
#[derive(Debug, Default, Clone)]
pub struct JsonCodec;

impl CacheCodec for JsonCodec {
    fn serialize<T: Serialize + ?Sized>(&self, value: &T) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(value)?)
    }

    fn deserialize<T: DeserializeOwned>(&self, bytes: &[u8]) -> Result<T> {
        Ok(serde_json::from_slice(bytes)?)
    }

    fn name(&self) -> &'static str {
        "serde_json"
    }
}
