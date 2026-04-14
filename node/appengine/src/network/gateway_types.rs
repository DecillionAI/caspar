#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum VmRuntimeType {
    Docker,
    Fire,
    Elpian,
    Elpify,
    Javascript,
    Wasm,
}

impl VmRuntimeType {
    fn from_str(v: &str) -> Option<Self> {
        match v.trim().to_lowercase().as_str() {
            "docker" => Some(Self::Docker),
            "fire" => Some(Self::Fire),
            "elpian" => Some(Self::Elpian),
            "elpify" => Some(Self::Elpify),
            "javascript" => Some(Self::Javascript),
            "wasm" => Some(Self::Wasm),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GatewayProtocol {
    Http,
    WebSocket,
    RawSocket,
}

impl GatewayProtocol {
    fn from_str(v: &str) -> Option<Self> {
        match v.trim().to_lowercase().as_str() {
            "http" | "https" => Some(Self::Http),
            "websocket" | "ws" | "wss" => Some(Self::WebSocket),
            "raw" | "socket" | "tcp" => Some(Self::RawSocket),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct VmGatewayEndpoint {
    machine_id: String,
    vm_id: String,
    runtime: VmRuntimeType,
    host: String,
    http_port: Option<u16>,
    websocket_port: Option<u16>,
    raw_socket_port: Option<u16>,
}

impl VmGatewayEndpoint {
    fn key(&self) -> String {
        format!("{}::{}", self.machine_id, self.vm_id)
    }

    fn protocol_port(&self, protocol: GatewayProtocol) -> Option<u16> {
        match protocol {
            GatewayProtocol::Http => self.http_port,
            GatewayProtocol::WebSocket => self.websocket_port,
            GatewayProtocol::RawSocket => self.raw_socket_port,
        }
    }
}

#[derive(Clone, Debug)]
struct GatewayForwardRequest {
    machine_id: String,
    vm_id: String,
    protocol: GatewayProtocol,
    path: String,
    method: String,
    body: Vec<u8>,
    headers: HashMap<String, String>,
}
