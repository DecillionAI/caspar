//! Translation of `drivers/network/client/tcp/tcp.go`.
//!
//! `Tcp` implements [`ITcp`] — the TLS-TCP server that user clients connect
//! to. Each accepted connection runs on its own thread; outbound writes are
//! buffered and acked one-at-a-time (matching Go's `Buffer` + `Ack` flow
//! control). The wire format is the framing implemented by
//! [`crate::drivers::network::framing`].

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use dashmap::DashMap;
use serde_json::Value;

use crate::abstractions::adapters::network::network::TlsConfig;
use crate::abstractions::adapters::network::tcp::ITcp;
use crate::abstractions::adapters::signaler::Listener;
use crate::abstractions::models::core::ICore;
use crate::abstractions::models::packet::{build_error_json, ResponseSimpleMessage};
use crate::abstractions::models::trx::ITrx;
use crate::drivers::network::framing::{
    accept, bind_tls, decode_request_body, encode_client_response_body, encode_client_update_body,
    read_length_prefixed_frame, write_length_prefixed_frame, TlsStream,
};
use crate::shell::utils::crypto::secure_unique_string;

/// Per-connection state shared between the connection handler and the
/// signaler-backed update push path.
pub struct Socket {
    pub id: String,
    inner: Mutex<SocketInner>,
}

struct SocketInner {
    stream: Option<TlsStream>,
    buffer: Vec<Vec<u8>>,
    ack: bool,
    disconnected: bool,
    user_id: String,
}

impl Socket {
    fn new(stream: TlsStream) -> Arc<Socket> {
        Arc::new(Socket {
            id: secure_unique_string(),
            inner: Mutex::new(SocketInner {
                stream: Some(stream),
                buffer: Vec::new(),
                ack: true,
                disconnected: false,
                user_id: String::new(),
            }),
        })
    }

    fn peer_ip(&self) -> String {
        let inner = self.inner.lock().unwrap();
        inner
            .stream
            .as_ref()
            .map(|s| {
                let addr = s.peer_addr();
                addr.rsplit_once(':').map(|(a, _)| a.to_string()).unwrap_or(addr)
            })
            .unwrap_or_default()
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

/// `Tcp` driver implementing [`ITcp`].
pub struct Tcp {
    app: Arc<dyn ICore>,
    sockets: Arc<DashMap<String, Arc<Socket>>>,
}

impl Tcp {
    /// `NewTcp(app)`.
    pub fn new(app: Arc<dyn ICore>) -> Arc<Tcp> {
        Arc::new(Tcp {
            app,
            sockets: Arc::new(DashMap::new()),
        })
    }

    fn process_inbound(self: &Arc<Self>, socket: &Arc<Socket>, body: Vec<u8>) {
        // 0x01 single-byte ack from client.
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
                if ok {
                    self.app
                        .tools()
                        .signaler()
                        .listeners()
                        .remove(&parsed.user_id);
                    socket.write_response(
                        &parsed.packet_id,
                        0,
                        &serde_json::to_vec(&build_error_json("loggedout")).unwrap_or_default(),
                    );
                } else {
                    socket.write_response(
                        &parsed.packet_id,
                        0,
                        &serde_json::to_vec(&build_error_json("logout_failed")).unwrap_or_default(),
                    );
                }
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
        let input = match secure.parse_input("tcp", raw_payload) {
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

        // Join every store the user has access to.
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
        let socket = Socket::new(stream);
        let peer_key = socket.peer_ip();
        if !peer_key.is_empty() {
            self.sockets.insert(peer_key, socket.clone());
        }

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
            self.process_inbound(&socket, frame);
        }

        // Clean up after disconnect — wait 60 s in case the user reconnects.
        let user_id;
        {
            let mut inner = socket.inner.lock().unwrap();
            inner.disconnected = true;
            user_id = inner.user_id.clone();
            if let Some(stream) = inner.stream.as_mut() {
                stream.shutdown();
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

impl ITcp for Tcp {
    fn listen(&self, port: i64, tls_config: Option<TlsConfig>) {
        let trans_self = Arc::new(self.clone_for_listen());
        thread::spawn(move || {
            let cfg_ref = tls_config.as_ref();
            let (listener, server) = match bind_tls(port, cfg_ref) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("tcp listen :{}: {}", port, e);
                    return;
                }
            };
            loop {
                let stream = match accept(&listener, server.as_ref()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("tcp accept: {}", e);
                        continue;
                    }
                };
                let trans = trans_self.clone();
                thread::spawn(move || trans.handle_connection(stream));
            }
        });
    }
}

impl Tcp {
    /// Internal: rebuild a fresh `Tcp` sharing the same maps and core so
    /// `listen()`'s `&self` can hand off ownership to the listener thread.
    fn clone_for_listen(&self) -> Tcp {
        Tcp {
            app: self.app.clone(),
            sockets: self.sockets.clone(),
        }
    }
}
