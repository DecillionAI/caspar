//! Translation of `abstract/models/trx/helpers.go`.

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};

/// Marshal an arbitrary object then unmarshal it into a JSON object map.
pub fn object_to_map<T: Serialize>(obj: &T) -> Result<Map<String, Value>> {
    let data = serde_json::to_vec(obj)?;
    let m: Map<String, Value> = serde_json::from_slice(&data)?;
    Ok(m)
}

/// Marshal a JSON object map then unmarshal it into a concrete object.
///
/// Go's `MapToObject` mutates the destination in place; the Rust translation
/// returns the decoded value instead.
pub fn map_to_object<T: DeserializeOwned>(m: &Map<String, Value>) -> Result<T> {
    let data = serde_json::to_vec(m)?;
    let obj: T = serde_json::from_slice(&data)?;
    Ok(obj)
}
