//! Translation of `drivers/network/client/tcp/tcp.go`.
//!
//! `Tcp` implements [`ITcp`] — the TLS-TCP server that user clients connect
//! to. Each accepted connection runs on its own thread; outbound writes are
//! buffered and acked one-at-a-time (matching Go's `Buffer` + `Ack` flow
//! control). The wire format is the framing implemented by
//! [`crate::drivers::network::framing`].
//!
//! ## Concurrency model
//!
//! Each connection's `TlsStream` is pinned to a single dedicated I/O thread
//! that performs both reads and writes. External writers push outbound
//! frames onto a per-socket `mpsc::Sender`, never touching the stream
//! directly — this avoids the well-known dead-lock where holding the stream
//! mutex across a blocking `read()` starves writers (e.g. a creature signal
//! result emitted from a wasm thread while the client is idle waiting for
//! it). See `ws.rs` for the same design applied to WebSocket connections.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use dashmap::DashMap;
use serde_json::Value;

use crate::drivers::network::framing::{
    accept, bind_tls, decode_request_body, encode_client_response_body, encode_client_update_body,
    write_length_prefixed_frame, TlsStream,
};
use crate::models::core::ICore;
use crate::models::packet::{build_error_json, ResponseSimpleMessage};
use crate::models::ports::network::tcp::ITcp;
use crate::models::ports::network::TlsConfig;
use crate::models::ports::signaler::Listener;
use crate::models::transaction::ITrx;
use crate::shell::utils::crypto::secure_unique_string;

/// Items the I/O thread accepts from external writers.
enum OutboundFrame {
    Body(Vec<u8>),
    Shutdown,
}

/// Per-connection state shared with the rest of the node. The owning TLS
/// stream lives inside the I/O thread; everything reachable through this
/// struct is safe to call from arbitrary threads.
pub struct Socket {
    pub id: String,
    peer: String,
    user_id: Mutex<String>,
    disconnected: AtomicBool,
    outbound: Mutex<Option<Sender<OutboundFrame>>>,
}

impl Socket {
    fn new(peer: String, outbound: Sender<OutboundFrame>) -> Arc<Socket> {
        Arc::new(Socket {
            id: secure_unique_string(),
            peer,
            user_id: Mutex::new(String::new()),
            disconnected: AtomicBool::new(false),
            outbound: Mutex::new(Some(outbound)),
        })
    }

    fn peer_ip(&self) -> String {
        self.peer
            .rsplit_once(':')
            .map(|(a, _)| a.to_string())
            .unwrap_or_else(|| self.peer.clone())
    }

    fn user_id(&self) -> String {
        self.user_id.lock().unwrap().clone()
    }

    fn set_user_id(&self, id: &str) {
        *self.user_id.lock().unwrap() = id.to_string();
    }

    fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }

    fn enqueue(&self, frame: Vec<u8>) {
        if self.is_disconnected() {
            return;
        }
        let guard = self.outbound.lock().unwrap();
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(OutboundFrame::Body(frame));
        }
    }

    fn write_update(&self, key: &str, payload: &[u8]) {
        let frame = encode_client_update_body(key, payload);
        self.enqueue(frame);
    }

    fn write_response(&self, packet_id: &str, res_code: i64, payload: &[u8]) {
        let frame = encode_client_response_body(packet_id, res_code, payload);
        self.enqueue(frame);
    }

    fn shutdown(&self) {
        if self.disconnected.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut guard = self.outbound.lock().unwrap();
        if let Some(tx) = guard.take() {
            let _ = tx.send(OutboundFrame::Shutdown);
        }
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
        // ACKs are handled inline by the I/O loop, so anything that reaches
        // here is a real request body.
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
                        &serde_json::to_vec(&build_error_json("logout_failed"))
                            .unwrap_or_default(),
                    );
                }
                return;
            }
            "authenticate" | "/creatures/authenticate" => {
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
                        &serde_json::to_vec(&build_error_json("authenticated"))
                            .unwrap_or_default(),
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
                        &serde_json::to_vec(&build_error_json("authentication failed"))
                            .unwrap_or_default(),
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
                    &serde_json::to_vec(&build_error_json("action not found"))
                        .unwrap_or_default(),
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
                    &serde_json::to_vec(&build_error_json(&format!("{}", e)))
                        .unwrap_or_default(),
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
                // Lazily register the update-stream listener after the first
                // successful authenticated request. This means callers don't
                // need a separate "authenticate" round-trip before signals
                // from WASM creatures can be delivered to them.
                if !parsed.user_id.is_empty()
                    && !self
                        .app
                        .tools()
                        .signaler()
                        .listeners()
                        .contains_key(&parsed.user_id)
                {
                    self.attach_user_listener(socket, &parsed.user_id);
                }
            }
            Err(e) => {
                socket.write_response(
                    &parsed.packet_id,
                    3,
                    &serde_json::to_vec(&build_error_json(&format!("{}", e)))
                        .unwrap_or_default(),
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
        socket.set_user_id(user_id);
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

    fn handle_connection(self: Arc<Self>, mut stream: TlsStream) {
        let peer = stream.peer_addr();
        // Short read timeout so the I/O loop can drain the outbound channel
        // without ever blocking writers.
        let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));

        let (tx, rx) = mpsc::channel::<OutboundFrame>();
        let socket = Socket::new(peer.clone(), tx);
        let peer_key = socket.peer_ip();
        if !peer_key.is_empty() {
            self.sockets.insert(peer_key, socket.clone());
        }

        // Resumable read accumulator. The legacy `read_length_prefixed_frame`
        // does a blocking `read_exact`, which on a timeout returns an error
        // and prevents us from getting back to the outbound drain. We do the
        // length/body read manually here, retaining partial state across
        // ticks so a timeout in the middle of a frame is harmless.
        let mut len_buf = [0u8; 4];
        let mut len_filled = 0usize;
        let mut body_buf: Vec<u8> = Vec::new();
        let mut body_filled = 0usize;
        let mut expected_len: Option<u32> = None;
        const MAX_FRAME: u32 = 32 * 1024 * 1024;

        let mut buffered: VecDeque<Vec<u8>> = VecDeque::new();
        let mut ack_ready = true;
        let mut shutdown_requested = false;

        'io: loop {
            // 1) Drain outbound channel.
            loop {
                match rx.try_recv() {
                    Ok(OutboundFrame::Body(b)) => buffered.push_back(b),
                    Ok(OutboundFrame::Shutdown) => shutdown_requested = true,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        shutdown_requested = true;
                        break;
                    }
                }
            }

            if shutdown_requested && buffered.is_empty() {
                break 'io;
            }

            // 2) Send next frame if ACK gate is open.
            // Update frames (tag 0x01) are fire-and-forget — the client does
            // not ACK them, so we pop them immediately and leave ack_ready
            // unchanged. Response frames (tag 0x02) gate on ACK so we keep
            // ack_ready=false until the client confirms receipt.
            if ack_ready {
                if let Some(frame) = buffered.front() {
                    let is_update = frame.first().copied() == Some(0x01);
                    match write_length_prefixed_frame(&mut stream, frame) {
                        Ok(()) => {
                            if is_update {
                                buffered.pop_front();
                                // ack_ready stays true — no ACK coming for updates
                            } else {
                                ack_ready = false;
                            }
                        }
                        Err(_) => break 'io,
                    }
                }
            }

            // 3) Try to read inbound bytes. Partial reads are absorbed into
            // the accumulator; a complete frame is dispatched.
            let read_target = if expected_len.is_none() {
                let target = &mut len_buf[len_filled..];
                read_into(&mut stream, target)
            } else {
                let target = &mut body_buf[body_filled..];
                read_into(&mut stream, target)
            };

            match read_target {
                ReadOutcome::Bytes(n) => {
                    if expected_len.is_none() {
                        len_filled += n;
                        if len_filled == 4 {
                            let len = u32::from_be_bytes(len_buf);
                            if len > MAX_FRAME {
                                break 'io;
                            }
                            expected_len = Some(len);
                            body_buf = vec![0u8; len as usize];
                            body_filled = 0;
                            len_filled = 0;
                            if len == 0 {
                                // Empty frame — dispatch immediately.
                                let frame = std::mem::take(&mut body_buf);
                                expected_len = None;
                                self.dispatch_frame(&socket, frame, &mut ack_ready, &mut buffered);
                            }
                        }
                    } else {
                        body_filled += n;
                        if body_filled == body_buf.len() {
                            let frame = std::mem::take(&mut body_buf);
                            expected_len = None;
                            body_filled = 0;
                            self.dispatch_frame(&socket, frame, &mut ack_ready, &mut buffered);
                        }
                    }
                }
                ReadOutcome::Idle => {
                    // Loop back to outbound drain.
                }
                ReadOutcome::Closed | ReadOutcome::Error => break 'io,
            }
        }

        socket.shutdown();
        stream.shutdown();
        let user_id = socket.user_id();
        if user_id.is_empty() {
            return;
        }
        let trans = self.clone();
        let socket_clone = socket.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(60));
            if !socket_clone.is_disconnected() {
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

    /// Dispatch a fully-read frame. ACKs flow back into the buffer gate so
    /// the next outbound frame can be sent; everything else goes through the
    /// regular action pipeline.
    fn dispatch_frame(
        self: &Arc<Self>,
        socket: &Arc<Socket>,
        body: Vec<u8>,
        ack_ready: &mut bool,
        buffered: &mut VecDeque<Vec<u8>>,
    ) {
        if body.len() == 1 && body[0] == 0x01 {
            *ack_ready = true;
            if !buffered.is_empty() {
                buffered.pop_front();
            }
            return;
        }
        self.process_inbound(socket, body);
    }
}

enum ReadOutcome {
    Bytes(usize),
    Idle,
    Closed,
    Error,
}

fn read_into(stream: &mut TlsStream, buf: &mut [u8]) -> ReadOutcome {
    use std::io::Read;
    if buf.is_empty() {
        return ReadOutcome::Idle;
    }
    match stream.read(buf) {
        Ok(0) => ReadOutcome::Closed,
        Ok(n) => ReadOutcome::Bytes(n),
        Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
            ReadOutcome::Idle
        }
        Err(e) if e.kind() == ErrorKind::Interrupted => ReadOutcome::Idle,
        Err(_) => ReadOutcome::Error,
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
