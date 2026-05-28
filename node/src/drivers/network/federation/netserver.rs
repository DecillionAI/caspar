//! Translation of `drivers/network/federation/netserver.go`.
//!
//! Listening side of the federation channel — accept TLS TCP connections,
//! parse `OriginPacket`s from the framed wire format, and dispatch them to
//! a `FedApi` bridge supplied by [`crate::drivers::network::federation::FedNet`].

use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::Result;
use dashmap::DashMap;

use crate::models::ports::network::TlsConfig;
use crate::models::core::ICore;
use crate::models::packet::OriginPacket;
use crate::drivers::network::framing::{
    accept, bind_tls, decode_request_body, decode_response_body, decode_update_body, dial,
    encode_fed_response_body, encode_fed_update_body, encode_request_body,
    read_length_prefixed_frame, write_length_prefixed_frame, TlsStream,
};
use crate::shell::utils::crypto::secure_unique_string;

/// Per-connection socket. Each `Socket` is owned by one read loop and shared
/// with whatever writer (federation request / response / update) needs to
/// push a frame down it.
pub struct Socket {
    pub id: String,
    inner: Mutex<SocketInner>,
}

struct SocketInner {
    stream: Option<TlsStream>,
    buffer: Vec<Vec<u8>>,
    ack: bool,
    peer: String,
}

impl Socket {
    fn new(stream: TlsStream) -> Arc<Socket> {
        Arc::new(Socket {
            id: secure_unique_string(),
            inner: Mutex::new(SocketInner {
                peer: stream.peer_addr(),
                stream: Some(stream),
                buffer: Vec::new(),
                ack: true,
            }),
        })
    }

    fn peer_ip(&self) -> String {
        let inner = self.inner.lock().unwrap();
        inner
            .peer
            .rsplit_once(':')
            .map(|(a, _)| a.to_string())
            .unwrap_or(inner.peer.clone())
    }

    fn push_buffer(&self, inner: &mut SocketInner) {
        if !inner.ack {
            return;
        }
        let Some(stream) = inner.stream.as_mut() else {
            return;
        };
        let Some(frame) = inner.buffer.first().cloned() else {
            return;
        };
        inner.ack = false;
        if write_length_prefixed_frame(stream, &frame).is_err() {
            inner.ack = true;
        }
    }

    fn enqueue(&self, frame: Vec<u8>) {
        let mut inner = self.inner.lock().unwrap();
        inner.buffer.push(frame);
        self.push_buffer(&mut inner);
    }

    /// Public outbound encoders — used by `FedNet` and tests.
    pub fn write_request(
        &self,
        request_id: &str,
        user_id: &str,
        path: &str,
        payload: &[u8],
        signature: &str,
    ) {
        let frame = encode_request_body(signature, user_id, path, request_id, payload);
        self.enqueue(frame);
    }

    pub fn write_response(
        &self,
        request_id: &str,
        res_code: i64,
        signature: &str,
        payload: &[u8],
    ) {
        let frame = encode_fed_response_body(request_id, res_code, signature, payload);
        self.enqueue(frame);
    }

    pub fn write_update(
        &self,
        key: &str,
        target_type: &str,
        target_id_val: &str,
        exceptions: &[String],
        signature: &str,
        payload: &[u8],
    ) {
        let target_id = format!("{}::{}", target_type, target_id_val);
        let frame = encode_fed_update_body(signature, &target_id, exceptions, key, payload);
        self.enqueue(frame);
    }
}

/// Bridge callback signature: `func(socket, srcIp, OriginPacket)`.
pub type FedApi =
    Arc<dyn Fn(Arc<Socket>, String, OriginPacket) + Send + Sync>;

/// Federation TLS-TCP server.
pub struct Tcp {
    app: Arc<dyn ICore>,
    bridge: Mutex<Option<FedApi>>,
    sockets: Arc<DashMap<String, Arc<Socket>>>,
}

impl Tcp {
    pub fn new(app: Arc<dyn ICore>) -> Arc<Tcp> {
        Arc::new(Tcp {
            app,
            bridge: Mutex::new(None),
            sockets: Arc::new(DashMap::new()),
        })
    }

    pub fn inject_bridge(&self, bridge: FedApi) {
        *self.bridge.lock().unwrap() = Some(bridge);
    }

    /// Open a new outbound socket toward `dest_address`. Returns `None` if
    /// the TLS handshake fails.
    pub fn new_outbound_socket(
        self: &Arc<Self>,
        dest_address: &str,
        tls_config: Option<&TlsConfig>,
    ) -> Option<Arc<Socket>> {
        let mut cfg = tls_config.cloned();
        if let Some(ref mut c) = cfg {
            if c.server_name.is_empty() {
                c.server_name = dest_address
                    .split(':')
                    .next()
                    .unwrap_or(dest_address)
                    .to_string();
            }
        }
        // Bounded retry with exponential back-off. Federation peers may be
        // briefly unreachable during container restart / cluster bootstrap;
        // a single hard failure here would propagate as a dropped request,
        // which the higher-level caller treats as a peer-down event and
        // never retries. The retry is bounded so a genuinely-dead peer
        // does not block the federation worker indefinitely — the
        // remaining peers can still make progress while this one heals.
        const MAX_DIAL_ATTEMPTS: u32 = 4;
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 0..MAX_DIAL_ATTEMPTS {
            match dial(dest_address, cfg.as_ref()) {
                Ok(stream) => {
                    let socket = self.register_inbound(stream);
                    return Some(socket);
                }
                Err(e) => {
                    last_err = Some(e);
                    if attempt + 1 >= MAX_DIAL_ATTEMPTS {
                        break;
                    }
                    let backoff_ms = 250u64 * (1u64 << attempt); // 250, 500, 1000ms
                    std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                }
            }
        }
        eprintln!(
            "federation dial {}: giving up after {} attempts ({})",
            dest_address,
            MAX_DIAL_ATTEMPTS,
            last_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "no error captured".into()),
        );
        None
    }

    fn register_inbound(self: &Arc<Self>, stream: TlsStream) -> Arc<Socket> {
        let socket = Socket::new(stream);
        let ip = socket.peer_ip();
        if !ip.is_empty() {
            self.sockets.insert(ip, socket.clone());
        }
        let trans = self.clone();
        let sock = socket.clone();
        thread::spawn(move || trans.listen_for_packets(sock));
        socket
    }

    fn listen_for_packets(self: Arc<Self>, socket: Arc<Socket>) {
        loop {
            let frame = {
                let mut inner = socket.inner.lock().unwrap();
                let s = match inner.stream.as_mut() {
                    Some(s) => s,
                    None => break,
                };
                match read_length_prefixed_frame(s) {
                    Ok(Some(b)) => b,
                    Ok(None) | Err(_) => break,
                }
            };
            let trans = self.clone();
            let sock = socket.clone();
            trans.process_packet(sock, frame);
        }
        let mut inner = socket.inner.lock().unwrap();
        if let Some(s) = inner.stream.as_mut() {
            s.shutdown();
        }
        let id_ip = inner
            .peer
            .rsplit_once(':')
            .map(|(a, _)| a.to_string())
            .unwrap_or(inner.peer.clone());
        self.sockets.remove(&id_ip);
    }

    fn process_packet(self: Arc<Self>, socket: Arc<Socket>, body: Vec<u8>) {
        if body == b"packet_received" {
            let mut inner = socket.inner.lock().unwrap();
            inner.ack = true;
            if !inner.buffer.is_empty() {
                inner.buffer.remove(0);
                socket.push_buffer(&mut inner);
            }
            return;
        }
        if body.is_empty() {
            return;
        }
        let pack = match body[0] {
            0x01 => {
                let frame = match decode_update_body(&body[1..], true) {
                    Ok(p) => p,
                    Err(_) => return,
                };
                let mut user_id = String::new();
                let mut store_id = String::new();
                if let Some(("user", id)) = frame.target_id.split_once("::") {
                    user_id = id.to_string();
                } else if let Some(("store", id)) = frame.target_id.split_once("::") {
                    store_id = id.to_string();
                }
                OriginPacket {
                    typ: "update".into(),
                    key: frame.key,
                    user_id,
                    store_id,
                    res_code: 0,
                    binary: frame.payload,
                    signature: frame.signature,
                    request_id: String::new(),
                    exceptions: frame.exceptions,
                }
            }
            0x02 => {
                let frame = match decode_response_body(&body[1..], true) {
                    Ok(p) => p,
                    Err(_) => return,
                };
                OriginPacket {
                    typ: "response".into(),
                    key: String::new(),
                    user_id: String::new(),
                    store_id: String::new(),
                    res_code: frame.res_code,
                    binary: frame.payload,
                    signature: frame.signature,
                    request_id: frame.packet_id,
                    exceptions: Vec::new(),
                }
            }
            0x03 => {
                let frame = match decode_request_body(&body[1..]) {
                    Ok(p) => p,
                    Err(_) => return,
                };
                OriginPacket {
                    typ: "request".into(),
                    key: frame.path,
                    user_id: frame.user_id,
                    store_id: String::new(),
                    res_code: 0,
                    binary: frame.payload,
                    signature: frame.signature,
                    request_id: frame.packet_id,
                    exceptions: Vec::new(),
                }
            }
            _ => return,
        };

        let bridge = self.bridge.lock().unwrap().clone();
        if let Some(bridge) = bridge {
            bridge(socket.clone(), socket.peer_ip(), pack);
        }
    }

    /// Begin the TLS-listen loop on `port`.
    pub fn listen(self: &Arc<Self>, port: i64, tls_config: Option<TlsConfig>) {
        let trans = self.clone();
        thread::spawn(move || {
            let cfg_ref = tls_config.as_ref();
            let (listener, server) = match bind_tls(port, cfg_ref) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("federation listen :{}: {}", port, e);
                    return;
                }
            };
            loop {
                let stream = match accept(&listener, server.as_ref()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("federation accept: {}", e);
                        continue;
                    }
                };
                trans.register_inbound(stream);
            }
        });
    }
}

// Suppress unused-results lint helper.
#[allow(dead_code)]
fn _force_use() -> Result<()> {
    Ok(())
}
