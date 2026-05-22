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

/// Serde helpers that (de)serialize `Vec<Vec<u8>>` as a JSON array of base64
/// strings, matching Go's `encoding/json` behaviour for `[][]byte` fields.
pub mod bytes_base64_vec {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::ser::SerializeSeq;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[Vec<u8>], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(v.len()))?;
        for item in v {
            seq.serialize_element(&STANDARD.encode(item))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<u8>>, D::Error> {
        let opt = Option::<Vec<String>>::deserialize(d)?;
        match opt {
            None => Ok(Vec::new()),
            Some(strs) => strs
                .iter()
                .map(|s| STANDARD.decode(s).map_err(serde::de::Error::custom))
                .collect(),
        }
    }
}

/// Clones a `OnceLock`, preserving an already-initialised value. Used to make
/// translated structs that carry lazy caches cloneable.
pub fn clone_once_lock<T: Clone>(o: &std::sync::OnceLock<T>) -> std::sync::OnceLock<T> {
    let new = std::sync::OnceLock::new();
    if let Some(v) = o.get() {
        let _ = new.set(v.clone());
    }
    new
}

/// Convenience alias mirroring Go's `error` value type.
pub type GoError = anyhow::Error;

/// Opaque value, the translation of Go's empty interface `interface{}` / `any`
/// when it is used for dynamic, downcastable values rather than JSON payloads.
pub type AnyVal = std::sync::Arc<dyn std::any::Any + Send + Sync>;
