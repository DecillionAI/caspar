mod prelude;
pub mod globals;
pub mod models;
pub mod bridge;
pub mod controllers;
pub mod host;
pub mod network;

use serde_json::Value as JsonValue;

pub fn dispatch_packet(packet: &JsonValue) -> String {
    bridge::vm_packet_router::route_vm_packet(packet)
}
