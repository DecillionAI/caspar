//! Translation of `drivers/vmm` — the virtual-machine driver.
//!
//! Monolithic in-process VM driver.
//!
//! VM control and host calls are routed as in-memory JSON packets through
//! [`dispatch_packet`] and handled by the bridge packet router.

mod prelude;
pub mod bootstrap;
pub mod bridge;
pub mod controllers;
pub mod driver;
pub mod globals;
pub mod host;
pub mod hostcall_entities;
pub mod hostcall_global;
pub mod hostcall_logs;
pub mod models;
pub mod network;

use serde_json::Value as JsonValue;

pub use driver::Vmm;

pub fn dispatch_packet(packet: &JsonValue) -> String {
    bridge::vm_packet_router::route_vm_packet(packet)
}
