//! Shell-API surface — action handlers, packet schemas, persisted models,
//! and federation update payloads.

pub mod actions;
#[path = "main.rs"]
pub mod main_api;
pub mod model;
pub mod packets;
pub mod updates;
