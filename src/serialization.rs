use crate::error::CacheResult;
use bytes::Bytes;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

/// High-performance cache data serialization enum
#[derive(Debug, Clone)]
pub enum CacheSerializer {
    /// Default JSON serializer
    Json(JsonSerializer),
    /// Binary serializer using bincode
    #[cfg(feature = "bincode")]
    Bincode(BincodeSerializer),
    /// Binary serializer using MessagePack
    #[cfg(feature = "msgpack")]
    MsgPack(MsgPackSerializer),
}

impl Default for CacheSerializer {
    fn default() -> Self {
        Self::Json(JsonSerializer)
    }
}

impl CacheSerializer {
    /// Serialize a value to Bytes
    pub fn serialize<T: Serialize>(&self, value: &T) -> CacheResult<Bytes> {
        match self {
            Self::Json(s) => s.serialize_internal(value),
            #[cfg(feature = "bincode")]
            Self::Bincode(s) => s.serialize_internal(value),
            #[cfg(feature = "msgpack")]
            Self::MsgPack(s) => s.serialize_internal(value),
        }
    }

    /// Deserialize Bytes to a value
    pub fn deserialize<T: DeserializeOwned>(&self, bytes: &[u8]) -> CacheResult<T> {
        match self {
            Self::Json(s) => s.deserialize_internal(bytes),
            #[cfg(feature = "bincode")]
            Self::Bincode(s) => s.deserialize_internal(bytes),
            #[cfg(feature = "msgpack")]
            Self::MsgPack(s) => s.deserialize_internal(bytes),
        }
    }

    /// Serializer identifier
    pub fn name(&self) -> &'static str {
        match self {
            Self::Json(_) => "Json",
            #[cfg(feature = "bincode")]
            Self::Bincode(_) => "Bincode",
            #[cfg(feature = "msgpack")]
            Self::MsgPack(_) => "MsgPack",
        }
    }
}

/// Default JSON serializer using serde_json
#[derive(Debug, Default, Clone)]
pub struct JsonSerializer;

impl JsonSerializer {
    fn serialize_internal<T: Serialize>(&self, value: &T) -> CacheResult<Bytes> {
        serde_json::to_vec(value)
            .map(Bytes::from)
            .map_err(|e| crate::error::CacheError::SerializationError(e.to_string()))
    }

    fn deserialize_internal<T: DeserializeOwned>(&self, bytes: &[u8]) -> CacheResult<T> {
        serde_json::from_slice(bytes)
            .map_err(|e| crate::error::CacheError::SerializationError(e.to_string()))
    }
}

/// Binary serializer using bincode
#[cfg(feature = "bincode")]
#[derive(Debug, Default, Clone)]
pub struct BincodeSerializer;

#[cfg(feature = "bincode")]
impl BincodeSerializer {
    fn serialize_internal<T: Serialize>(&self, value: &T) -> CacheResult<Bytes> {
        bincode::serialize(value)
            .map(Bytes::from)
            .map_err(|e| crate::error::CacheError::SerializationError(e.to_string()))
    }

    fn deserialize_internal<T: DeserializeOwned>(&self, bytes: &[u8]) -> CacheResult<T> {
        bincode::deserialize(bytes)
            .map_err(|e| crate::error::CacheError::SerializationError(e.to_string()))
    }
}

/// Binary serializer using MessagePack
#[cfg(feature = "msgpack")]
#[derive(Debug, Default, Clone)]
pub struct MsgPackSerializer;

#[cfg(feature = "msgpack")]
impl MsgPackSerializer {
    fn serialize_internal<T: Serialize>(&self, value: &T) -> CacheResult<Bytes> {
        rmp_serde::to_vec(value)
            .map(Bytes::from)
            .map_err(|e| crate::error::CacheError::SerializationError(e.to_string()))
    }

    fn deserialize_internal<T: DeserializeOwned>(&self, bytes: &[u8]) -> CacheResult<T> {
        rmp_serde::from_slice(bytes)
            .map_err(|e| crate::error::CacheError::SerializationError(e.to_string()))
    }
}
