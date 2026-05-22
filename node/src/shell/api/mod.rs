//! Translation of `shell/api` — request/response/update payload models and
//! the action handlers that turn them into state mutations.

pub mod actions;
pub mod inputs;
#[path = "main.rs"]
pub mod main_api;
pub mod model;
pub mod outputs;
pub mod updates;
