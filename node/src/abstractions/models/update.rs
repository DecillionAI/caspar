//! Translation of `abstract/models/update/update.go`.

use serde::{Deserialize, Serialize};

/// A single key/value mutation produced by a transaction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Update {
    #[serde(rename = "type")]
    pub typ: String,
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "val", with = "crate::util::bytes_base64", default)]
    pub val: Vec<u8>,
}
