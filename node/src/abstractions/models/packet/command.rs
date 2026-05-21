//! Translation of `abstract/models/packet/command.go`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Command {
    pub value: String,
    pub data: String,
}
