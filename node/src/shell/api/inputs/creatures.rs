//! Translation of `shell/api/inputs/creatures` — request payloads for the
//! creature lifecycle actions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::abstractions::models::input::IInput;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateInput {
    #[serde(rename = "type", default)]
    pub typ: String,
    #[serde(default)]
    pub username: String,
    #[serde(rename = "publicKey", default)]
    pub public_key: String,
    #[serde(rename = "chainId", default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    #[serde(rename = "subchainId", default, skip_serializing_if = "Option::is_none")]
    pub subchain_id: Option<String>,
    #[serde(rename = "ownerId", default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

impl IInput for CreateInput {
    fn get_store_id(&self) -> String { String::new() }
    fn origin(&self) -> String { "global".to_string() }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignalInput {
    #[serde(rename = "type", default)]
    pub typ: String,
    #[serde(default)]
    pub data: String,
    #[serde(rename = "storeId", default)]
    pub store_id: String,
    #[serde(rename = "creatureId", default)]
    pub creature_id: String,
    #[serde(rename = "programId", default, skip_serializing_if = "String::is_empty")]
    pub program_id: String,
    #[serde(rename = "entityId", default, skip_serializing_if = "String::is_empty")]
    pub entity_id: String,
    #[serde(default)]
    pub temp: bool,
}

impl IInput for SignalInput {
    fn get_store_id(&self) -> String { self.store_id.clone() }
    fn origin(&self) -> String { String::new() }
}
