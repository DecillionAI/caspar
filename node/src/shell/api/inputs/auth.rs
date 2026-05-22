//! Translation of `shell/api/inputs/auth`.

use serde::{Deserialize, Serialize};

use crate::abstractions::models::input::IInput;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetServerKeyInput {}

impl IInput for GetServerKeyInput {
    fn get_store_id(&self) -> String { String::new() }
    fn origin(&self) -> String { String::new() }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetServersMapInput {}

impl IInput for GetServersMapInput {
    fn get_store_id(&self) -> String { String::new() }
    fn origin(&self) -> String { String::new() }
}
