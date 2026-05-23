mod prelude;
pub mod globals;
pub mod models;
pub mod bridge;
pub mod controllers;
pub mod host;
pub mod network;

use serde_json::Value as JsonValue;

pub fn dispatch_packet(packet: &JsonValue) -> String {
    bridge::zmq_packet_dispatcher::dispatch_zmq_packet(packet)
}
