use crate::drivers::vmm::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GatewayProtocol {
    Http,
    WebSocket,
    RawSocket,
}

impl GatewayProtocol {
    pub(crate) fn from_str(v: &str) -> Option<Self> {
        match v.trim().to_lowercase().as_str() {
            "http" | "https" => Some(Self::Http),
            "websocket" | "ws" | "wss" => Some(Self::WebSocket),
            "raw" | "socket" | "tcp" => Some(Self::RawSocket),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct VmGatewayEndpoint {
    pub(crate) machine_id: String,
    pub(crate) vm_id: String,
    /// Canonical runtime key of the VM, validated against the VM plugin
    /// registry at registration time (no VM type is hardcoded here).
    pub(crate) runtime: String,
    pub(crate) host: String,
    pub(crate) http_port: Option<u16>,
    pub(crate) websocket_port: Option<u16>,
    pub(crate) raw_socket_port: Option<u16>,
}

impl VmGatewayEndpoint {
    pub(crate) fn key(&self) -> String {
        format!("{}::{}", self.machine_id, self.vm_id)
    }

    pub(crate) fn protocol_port(&self, protocol: GatewayProtocol) -> Option<u16> {
        match protocol {
            GatewayProtocol::Http => self.http_port,
            GatewayProtocol::WebSocket => self.websocket_port,
            GatewayProtocol::RawSocket => self.raw_socket_port,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GatewayForwardRequest {
    pub(crate) machine_id: String,
    pub(crate) vm_id: String,
    pub(crate) protocol: GatewayProtocol,
    pub(crate) path: String,
    pub(crate) method: String,
    pub(crate) body: Vec<u8>,
    pub(crate) headers: HashMap<String, String>,
}
