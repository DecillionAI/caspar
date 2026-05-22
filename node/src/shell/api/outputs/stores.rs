//! Translation of `shell/api/outputs/stores/compat.go`.

use serde::{Deserialize, Serialize};

use crate::shell::api::model::Store;

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
