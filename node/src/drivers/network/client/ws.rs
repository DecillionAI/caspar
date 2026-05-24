//! Translation of `drivers/network/client/ws/ws.go`.
//!
//! `Ws` implements [`IWs`] — a TLS WebSocket server using `tungstenite` for
//! the WS protocol. Each accepted WS connection runs the same authentication
//! / action-dispatch state machine as the TCP driver, but speaks the framing
//! over `Message::Binary(...)` frames instead of length-prefixed TCP frames.
//!
//! Go's original (lxzan/gws) wrote two messages per outbound payload — first
//! a 4-byte length and then the body. The Rust translation collapses that
//! into a single Binary message whose payload is `u32be(len) || body` so the
//! shape matches what the existing Go clients expect on the wire.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use dashmap::DashMap;
use serde_json::Value;
use tungstenite::protocol::Message;
use tungstenite::{accept as ws_accept, WebSocket};

use crate::abstractions::ports::network::TlsConfig;
use crate::abstractions::ports::network::ws::IWs;
use crate::abstractions::ports::signaler::Listener;
use crate::abstractions::models::core::ICore;
use crate::abstractions::models::packet::{build_error_json, ResponseSimpleMessage};
use crate::abstractions::models::trx::ITrx;
use crate::drivers::network::framing::{
    accept, bind_tls, decode_request_body, encode_client_response_body,
    encode_client_update_body, TlsStream,
};
use crate::shell::utils::crypto::secure_unique_string;

/// Per-WS-connection state.
pub struct Socket {
    pub id: String,
    inner: Mutex<SocketInner>,
}

struct SocketInner {
    ws: Option<WebSocket<TlsStream>>,
    buffer: Vec<Vec<u8>>,
    ack: bool,
    disconnected: bool,
    user_id: String,
    peer: String,
}

impl Socket {
    fn new(ws: WebSocket<TlsStream>, peer: String) -> Arc<Socket> {
        Arc::new(Socket {
            id: secure_unique_string(),
            inner: Mutex::new(SocketInner {
                ws: Some(ws),
                buffer: Vec::new(),
                ack: true,
                disconnected: false,
                user_id: String::new(),
                peer,
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
        let Some(ws) = inner.ws.as_mut() else {
            return;
        };
        let Some(frame) = inner.buffer.first().cloned() else {
            return;
        };
        inner.ack = false;
        let mut msg = Vec::with_capacity(4 + frame.len());
        msg.extend_from_slice(&(frame.len() as u32).to_be_bytes());
        msg.extend_from_slice(&frame);
        if ws.send(Message::Binary(msg.into())).is_err() {
            inner.ack = true;
        }
    }

    fn write_update(&self, key: &str, payload: &[u8]) {
        let frame = encode_client_update_body(key, payload);
        let mut inner = self.inner.lock().unwrap();
        inner.buffer.push(frame);
        self.push_buffer(&mut inner);
    }

    fn write_response(&self, packet_id: &str, res_code: i64, payload: &[u8]) {
        let frame = encode_client_response_body(packet_id, res_code, payload);
        let mut inner = self.inner.lock().unwrap();
        inner.buffer.push(frame);
        self.push_buffer(&mut inner);
    }
}

/// `Ws` driver implementing [`IWs`].
pub struct Ws {
    app: Arc<dyn ICore>,
    sockets: Arc<DashMap<String, Arc<Socket>>>,
}

impl Ws {
    /// `NewWs(app)`.
    pub fn new(app: Arc<dyn ICore>) -> Arc<Ws> {
        Arc::new(Ws {
            app,
            sockets: Arc::new(DashMap::new()),
        })
    }

    fn process_inbound(self: &Arc<Self>, socket: &Arc<Socket>, body: Vec<u8>) {
        // Strip Go's outer 4-byte length prefix (the gws client sends it as
        // part of the same Binary message).
        let body = if body.len() >= 4 { body[4..].to_vec() } else { body };

        if body.len() == 1 && body[0] == 0x01 {
            let mut inner = socket.inner.lock().unwrap();
            inner.ack = true;
            if !inner.buffer.is_empty() {
                inner.buffer.remove(0);
                socket.push_buffer(&mut inner);
            }
            return;
        }

        let parsed = match decode_request_body(&body) {
            Ok(p) => p,
            Err(_) => return,
        };
        let peer_ip = socket.peer_ip();

        match parsed.path.as_str() {
            "logout" => {
                let (ok, _, _) = self
                    .app
                    .tools()
                    .security()
                    .auth_with_signature(&parsed.user_id, &parsed.payload, &parsed.signature);
                let msg = if ok {
                    self.app
                        .tools()
                        .signaler()
                        .listeners()
                        .remove(&parsed.user_id);
                    "loggedout"
                } else {
                    "logout_failed"
                };
                socket.write_response(&parsed.packet_id, 0, &serde_json::to_vec(&build_error_json(msg)).unwrap_or_default());
                return;
            }
            "authenticate" => {
                let (ok, _, _) = self
                    .app
                    .tools()
                    .security()
                    .auth_with_signature(&parsed.user_id, &parsed.payload, &parsed.signature);
                if ok {
                    self.attach_user_listener(socket, &parsed.user_id);
                    socket.write_response(
                        &parsed.packet_id,
                        0,
                        &serde_json::to_vec(&build_error_json("authenticated")).unwrap_or_default(),
                    );
                    let msg = serde_json::to_vec(&ResponseSimpleMessage {
                        message: "old_queue_end".to_string(),
                    })
                    .unwrap_or_default();
                    socket.write_update("old_queue_end", &msg);
                } else {
                    socket.write_response(
                        &parsed.packet_id,
                        4,
                        &serde_json::to_vec(&build_error_json("authentication failed")).unwrap_or_default(),
                    );
                }
                return;
            }
            _ => {}
        }

        let secure = match self.app.actor().fetch_secure_action(&parsed.path) {
            Some(s) => s,
            None => {
                socket.write_response(
                    &parsed.packet_id,
                    1,
                    &serde_json::to_vec(&build_error_json("action not found")).unwrap_or_default(),
                );
                return;
            }
        };
        let raw_payload =
            serde_json::from_slice::<Value>(&parsed.payload).unwrap_or(Value::Null);
        let input = match secure.parse_input("ws", raw_payload) {
            Ok(i) => i,
            Err(e) => {
                socket.write_response(
                    &parsed.packet_id,
                    2,
                    &serde_json::to_vec(&build_error_json(&format!("{}", e))).unwrap_or_default(),
                );
                return;
            }
        };
        match secure.securely_act(
            &parsed.user_id,
            &parsed.packet_id,
            &parsed.payload,
            &parsed.signature,
            input,
            &peer_ip,
            &[],
        ) {
            Ok((sc, value)) => {
                let body = serde_json::to_vec(&value).unwrap_or_default();
                socket.write_response(&parsed.packet_id, sc, &body);
            }
            Err(e) => {
                socket.write_response(
                    &parsed.packet_id,
                    3,
                    &serde_json::to_vec(&build_error_json(&format!("{}", e))).unwrap_or_default(),
                );
            }
        }
    }

    fn attach_user_listener(self: &Arc<Self>, socket: &Arc<Socket>, user_id: &str) {
        let socket_clone = socket.clone();
        let listener = Arc::new(Listener {
            id: user_id.to_string(),
            paused: false,
            dis_time: 0,
            signal: Arc::new(move |key, value| {
                let bytes = serde_json::to_vec(&value).unwrap_or_default();
                socket_clone.write_update(&key, &bytes);
            }),
        });
        self.sockets.insert(user_id.to_string(), socket.clone());
        {
            let mut inner = socket.inner.lock().unwrap();
            inner.user_id = user_id.to_string();
        }
        self.app.tools().signaler().listen_to_single(listener);

        let prefix = format!("hasaccess::{}::", user_id);
        let store_ids = Arc::new(Mutex::new(Vec::<String>::new()));
        let store_clone = store_ids.clone();
        let prefix_owned = prefix.clone();
        self.app.modify_state(
            true,
            Box::new(move |trx: &dyn ITrx| {
                if let Ok(ids) = trx.get_links_list(&prefix_owned, -1, -1, &[]) {
                    *store_clone.lock().unwrap() = ids;
                }
                Ok(())
            }),
        );
        let ids = store_ids.lock().unwrap().clone();
        for id in ids {
            let store_id = id.strip_prefix(&prefix).unwrap_or(&id).to_string();
            self.app.tools().signaler().join_group(&store_id, user_id);
        }
    }

    fn handle_connection(self: Arc<Self>, stream: TlsStream) {
        let peer = stream.peer_addr();
        let ws = match ws_accept(stream) {
            Ok(ws) => ws,
            Err(e) => {
                eprintln!("ws handshake from {}: {}", peer, e);
                return;
            }
        };
        let socket = Socket::new(ws, peer.clone());
        if let Some((ip, _)) = peer.rsplit_once(':') {
            self.sockets.insert(ip.to_string(), socket.clone());
        }

        loop {
            let msg = {
                let mut inner = socket.inner.lock().unwrap();
                let ws = match inner.ws.as_mut() {
                    Some(ws) => ws,
                    None => break,
                };
                match ws.read() {
                    Ok(m) => m,
                    Err(_) => break,
                }
            };
            match msg {
                Message::Binary(bytes) => {
                    self.process_inbound(&socket, bytes.to_vec());
                }
                Message::Ping(payload) => {
                    let mut inner = socket.inner.lock().unwrap();
                    if let Some(ws) = inner.ws.as_mut() {
                        let _ = ws.send(Message::Pong(payload));
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }

        let user_id;
        {
            let mut inner = socket.inner.lock().unwrap();
            inner.disconnected = true;
            user_id = inner.user_id.clone();
            if let Some(mut ws) = inner.ws.take() {
                let _ = ws.close(None);
            }
        }
        if user_id.is_empty() {
            return;
        }
        let trans = self.clone();
        let socket_clone = socket.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(60));
            let still_disconnected = socket_clone.inner.lock().unwrap().disconnected;
            if !still_disconnected {
                return;
            }
            if let Some(current) = trans.sockets.get(&user_id) {
                if Arc::ptr_eq(&socket_clone, current.value()) {
                    trans.sockets.remove(&user_id);
                    trans.app.tools().signaler().listeners().remove(&user_id);
                }
            }
        });
    }
}

impl IWs for Ws {
    fn listen(&self, port: i64, tls_config: Option<TlsConfig>) {
        let trans_self = Arc::new(self.clone_for_listen());
        thread::spawn(move || {
            let cfg_ref = tls_config.as_ref();
            let (listener, server) = match bind_tls(port, cfg_ref) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("ws listen :{}: {}", port, e);
                    return;
                }
            };
            loop {
                let stream = match accept(&listener, server.as_ref()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("ws accept: {}", e);
                        continue;
                    }
                };
                let trans = trans_self.clone();
                thread::spawn(move || trans.handle_connection(stream));
            }
        });
    }
}

impl Ws {
    fn clone_for_listen(&self) -> Ws {
        Ws {
            app: self.app.clone(),
            sockets: self.sockets.clone(),
        }
    }
}
