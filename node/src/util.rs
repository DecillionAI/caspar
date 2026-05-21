//! Shared low-level helpers used across the translated Caspar node.

/// Serde helpers that (de)serialize `Vec<u8>` as a standard base64 string,
/// matching Go's `encoding/json` behaviour for `[]byte` fields.
pub mod bytes_base64 {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let opt = Option::<String>::deserialize(d)?;
        match opt {
            None => Ok(Vec::new()),
            Some(s) => STANDARD.decode(s.as_bytes()).map_err(serde::de::Error::custom),
        }
    }
}

/// Convenience alias mirroring Go's `error` value type.
pub type GoError = anyhow::Error;

/// Opaque value, the translation of Go's empty interface `interface{}` / `any`
/// when it is used for dynamic, downcastable values rather than JSON payloads.
pub type AnyVal = std::sync::Arc<dyn std::any::Any + Send + Sync>;
