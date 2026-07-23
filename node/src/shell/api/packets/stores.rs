//! Request, response, and federation-broadcast payloads for the `stores`
//! action namespace.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::input::IInput;
use crate::shell::api::model::{Creature, Store};

fn is_false(b: &bool) -> bool {
    !*b
}

fn store_is_empty(s: &Store) -> bool {
    s.id.is_empty() && s.tag.is_empty() && s.parent_id.is_empty()
}

// ---- Inputs ----------------------------------------------------------------

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

// ---- Outputs ---------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdminPoiint {
    #[serde(default)]
    pub store: Store,
    #[serde(default)]
    pub admin: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateOutput {
    #[serde(default)]
    pub store: AdminPoiint,
}

// ---- Updates ---------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Send {
    #[serde(default)]
    pub user: Creature,
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
    /// Correlation id carried across a proxy-entity round trip: the requester
    /// (or the proxy itself) stamps it on the forwarded signal, the target
    /// echoes it on its response signal, and the node uses it to route the
    /// response back through the proxy entity to the original sender.
    #[serde(
        rename = "correlationId",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub correlation_id: String,
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
    pub user: Creature,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateMember {
    #[serde(rename = "storeId", default)]
    pub store_id: String,
    #[serde(default)]
    pub user: Creature,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Join {
    #[serde(rename = "storeId", default)]
    pub store_id: String,
    #[serde(default)]
    pub user: Creature,
}
