//! Translation of `shell/api/inputs/stores`.

use serde::{Deserialize, Serialize};

use crate::abstractions::models::input::IInput;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignalInput {
    #[serde(rename = "type", default)]
    pub typ: String,
    #[serde(rename = "storeId", default)]
    pub store_id: String,
    #[serde(rename = "userId", default)]
    pub user_id: String,
    #[serde(default)]
    pub data: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub temp: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl IInput for SignalInput {
    fn get_store_id(&self) -> String {
        self.store_id.clone()
    }
    fn origin(&self) -> String {
        String::new()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JoinInput {
    #[serde(rename = "storeId", default)]
    pub store_id: String,
}
