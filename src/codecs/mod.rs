//! Cache Codec Implementations
//!
//! This module provides built-in implementations of the [`CacheCodec`](crate::traits::CacheCodec) trait
//! for different serialization backends.

mod json;
pub use json::JsonCodec;

#[cfg(feature = "simd_json")]
mod simd_json;
#[cfg(feature = "simd_json")]
#[cfg_attr(docsrs, doc(cfg(feature = "simd_json")))]
pub use simd_json::SimdJsonCodec;

#[cfg(feature = "postcard")]
mod postcard;
#[cfg(feature = "postcard")]
#[cfg_attr(docsrs, doc(cfg(feature = "postcard")))]
pub use postcard::PostcardCodec;
