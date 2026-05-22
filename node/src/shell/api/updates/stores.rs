//! Translation of `shell/api/updates/stores/compat.go`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::shell::api::model::{Store, User};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Send {
    #[serde(default)]
    pub user: User,
    #[serde(default, skip_serializing_if = "store_is_empty")]
    pub store: Store,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub data: String,
    #[serde(rename = "isTemp", default, skip_serializing_if = "is_false")]
    pub is_temp: bool,
    #[serde(rename = "entityId", default, skip_serializing_if = "String::is_empty")]
    pub entity_id: String,
}

fn store_is_empty(s: &Store) -> bool {
    s.id.is_empty() && s.tag.is_empty() && s.parent_id.is_empty()
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Update {
    #[serde(default)]
    pub store: Store,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Delete {
    #[serde(default)]
    pub store: Store,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddMember {
    #[serde(rename = "storeId", default)]
    pub store_id: String,
    #[serde(default)]
    pub user: User,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateMember {
    #[serde(rename = "storeId", default)]
    pub store_id: String,
    #[serde(default)]
    pub user: User,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Join {
    #[serde(rename = "storeId", default)]
    pub store_id: String,
    #[serde(default)]
    pub user: User,
}
