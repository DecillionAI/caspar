//! Translation of `shell/api/inputs/{empty,hello,ping}.go`.

use serde::{Deserialize, Serialize};

use crate::abstractions::models::input::IInput;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmptyInput {}

impl IInput for EmptyInput {
    fn get_store_id(&self) -> String { String::new() }
    fn origin(&self) -> String { String::new() }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HelloInput {
    #[serde(default)]
    pub name: String,
}

impl IInput for HelloInput {
    fn get_store_id(&self) -> String { String::new() }
    fn origin(&self) -> String { String::new() }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PingInput {}

impl IInput for PingInput {
    fn get_store_id(&self) -> String { String::new() }
    fn origin(&self) -> String { String::new() }
}
