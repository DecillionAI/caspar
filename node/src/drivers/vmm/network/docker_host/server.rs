//! The docker-host bridge gateway TCP server.
//!
//! A single listener accepts connections from docker creature containers. Each
//! connection is pinned to a dedicated I/O thread that owns its `TcpStream`; a
//! 20 ms read timeout lets that thread interleave inbound reads with draining
//! the outbound MPSC queue, so signal pushes and host-call responses never
//! block on, or deadlock against, the read path. This mirrors the proven
//! concurrency model of the federation `netserver`.
//!
//! Lifecycle of a connection:
//!
//! 1. container connects and sends `HELLO {vmId, machineId, creatureId,
//!    programId}`;
//! 2. the node registers it in [`GATEWAY_REGISTRY`] and replies `WELCOME`;
//! 3. the container streams `REQUEST` host-calls (handled concurrently) and
//!    receives `RESPONSE`s plus any pushed `SIGNAL`s;
//! 4. on disconnect the connection is removed from the registry.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use crate::drivers::vmm::network::docker_host::connection::{
    ContainerIdentity, GatewayConnection, OutboundFrame, GATEWAY_REGISTRY,
};
use crate::drivers::vmm::network::docker_host::dispatch::dispatch_host_call;
use crate::drivers::vmm::network::docker_host::protocol::{
    encode_json_message, encode_message, next_message_id, GatewayMessage, MessageAssembler, Opcode,
    MAX_FRAME,
};
use crate::drivers::vmm::prelude::*;

/// Start the gateway listener on `0.0.0.0:port` in a background thread.
///
/// A `port` of `0` or below disables the gateway (used when the deployment
/// doesn't run docker creatures). Safe to call once during node startup.
pub fn start(port: i64) {
    if port <= 0 {
        return;
    }
    thread::spawn(move || {
        let addr = format!("0.0.0.0:{}", port);
        let listener = match TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[docker-host-gateway] bind {} failed: {}", addr, e);
                return;
            }
        };
        eprintln!("[docker-host-gateway] listening on {}", addr);
        for incoming in listener.incoming() {
            match incoming {
                Ok(stream) => {
                    thread::spawn(move || run_connection(stream));
                }
                Err(e) => {
                    eprintln!("[docker-host-gateway] accept error: {}", e);
                    // Never abandon the listener over a single accept failure.
                    continue;
                }
            }
        }
    });
}

/// Per-connection I/O loop. Owns the `TcpStream` for the lifetime of the
/// connection.
fn run_connection(stream: TcpStream) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));
    let _ = stream.set_nodelay(true);
    let mut stream = stream;

    // All outbound frames — WELCOME/PONG/RESPONSE (from worker threads) and
    // pushed SIGNALs (from the registry) — funnel through this one channel.
    let (tx, rx): (Sender<OutboundFrame>, Receiver<OutboundFrame>) = mpsc::channel();

    let mut session: Option<Arc<GatewayConnection>> = None;
    let mut assembler = MessageAssembler::new();

    // Resumable length-prefix accumulator (identical scheme to netserver/tcp).
    let mut len_buf = [0u8; 4];
    let mut len_filled = 0usize;
    let mut body_buf: Vec<u8> = Vec::new();
    let mut body_filled = 0usize;
    let mut expected_len: Option<usize> = None;

    let mut outbound: VecDeque<Vec<u8>> = VecDeque::new();
    let mut shutdown_requested = false;

    'io: loop {
        // 1) Drain the outbound MPSC channel into the local queue.
        loop {
            match rx.try_recv() {
                Ok(OutboundFrame::Body(b)) => outbound.push_back(b),
                Ok(OutboundFrame::Shutdown) => shutdown_requested = true,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    shutdown_requested = true;
                    break;
                }
            }
        }

        if shutdown_requested && outbound.is_empty() {
            break 'io;
        }

        // 2) Flush pending outbound frames eagerly.
        while let Some(frame) = outbound.front() {
            match stream.write_all(frame) {
                Ok(()) => {
                    outbound.pop_front();
                }
                Err(_) => break 'io,
            }
        }

        // 3) Read inbound bytes (non-blocking, 20 ms timeout).
        let read_target: &mut [u8] = if expected_len.is_none() {
            &mut len_buf[len_filled..]
        } else {
            &mut body_buf[body_filled..]
        };

        match read_into(&mut stream, read_target) {
            ReadOutcome::Bytes(n) => {
                if expected_len.is_none() {
                    len_filled += n;
                    if len_filled == 4 {
                        let len = u32::from_be_bytes(len_buf) as usize;
                        len_filled = 0;
                        if len == 0 {
                            // Zero-length keep-alive frame — ignore.
                            continue;
                        }
                        if len > MAX_FRAME {
                            eprintln!(
                                "[docker-host-gateway] {} oversize frame {} — closing",
                                peer, len
                            );
                            break 'io;
                        }
                        expected_len = Some(len);
                        body_buf = vec![0u8; len];
                        body_filled = 0;
                    }
                } else {
                    body_filled += n;
                    if body_filled == body_buf.len() {
                        let body = std::mem::take(&mut body_buf);
                        expected_len = None;
                        body_filled = 0;
                        match assembler.push_frame(&body) {
                            Ok(Some(msg)) => {
                                if !handle_message(&peer, &tx, &mut session, msg) {
                                    break 'io;
                                }
                            }
                            Ok(None) => { /* awaiting more chunks */ }
                            Err(e) => {
                                eprintln!("[docker-host-gateway] {} frame error: {}", peer, e);
                                break 'io;
                            }
                        }
                    }
                }
            }
            ReadOutcome::Idle => { /* timeout — loop back to outbound drain */ }
            ReadOutcome::Closed | ReadOutcome::Error => break 'io,
        }
    }

    // Cleanup.
    if let Some(conn) = session.take() {
        conn.shutdown();
        GATEWAY_REGISTRY.remove(conn.conn_id);
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// Handle one fully-reassembled inbound message. Returns `false` to terminate
/// the connection.
fn handle_message(
    peer: &str,
    tx: &Sender<OutboundFrame>,
    session: &mut Option<Arc<GatewayConnection>>,
    msg: GatewayMessage,
) -> bool {
    match msg.opcode {
        Opcode::Hello => {
            if session.is_some() {
                // Duplicate handshake — ignore, keep the existing session.
                return true;
            }
            let body = msg.json();
            let identity = ContainerIdentity {
                vm_id: str_field(&body, "vmId"),
                machine_id: str_field(&body, "machineId"),
                creature_id: str_field(&body, "creatureId"),
                program_id: str_field(&body, "programId"),
            };
            if identity.vm_id.is_empty() && identity.program_id.is_empty() {
                send_local(
                    tx,
                    Opcode::Error,
                    0,
                    &json!({ "ok": false, "error": "HELLO requires vmId or programId" }),
                );
                return false;
            }
            let conn_id = GATEWAY_REGISTRY.alloc_conn_id();
            let conn = GatewayConnection::new(conn_id, peer.to_string(), identity.clone(), tx.clone());
            GATEWAY_REGISTRY.insert(conn.clone());
            *session = Some(conn);
            eprintln!(
                "[docker-host-gateway] {} registered vm={} machine={} (conns={})",
                peer,
                identity.vm_id,
                identity.machine_id,
                GATEWAY_REGISTRY.count()
            );
            send_local(
                tx,
                Opcode::Welcome,
                msg.correlation_id,
                &json!({ "ok": true, "sessionId": conn_id, "vmId": identity.vm_id }),
            );
            true
        }
        Opcode::Request => {
            let Some(conn) = session.clone() else {
                send_local(
                    tx,
                    Opcode::Error,
                    msg.correlation_id,
                    &json!({ "ok": false, "error": "send HELLO before REQUEST" }),
                );
                return false;
            };
            // Run the (potentially blocking) host call off the I/O thread so the
            // read loop stays responsive and host calls can run concurrently.
            let tx = tx.clone();
            let correlation_id = msg.correlation_id;
            let request = msg.json();
            let identity = conn.identity.clone();
            thread::spawn(move || {
                let bytes = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    dispatch_host_call(&identity, &request)
                }))
                .unwrap_or_else(|_| {
                    json!({ "ok": false, "error": "host call panicked" })
                        .to_string()
                        .into_bytes()
                });
                let frames =
                    encode_message(Opcode::Response, next_message_id(), correlation_id, &bytes);
                for frame in frames {
                    if tx.send(OutboundFrame::Body(frame)).is_err() {
                        break;
                    }
                }
            });
            true
        }
        Opcode::Ping => {
            send_local(tx, Opcode::Pong, msg.correlation_id, &json!({ "ok": true }));
            true
        }
        // Containers never send these; ignore defensively.
        Opcode::Welcome | Opcode::Response | Opcode::Signal | Opcode::Pong | Opcode::Error => true,
    }
}

/// Encode a control message and enqueue it on this connection's outbound queue.
fn send_local(tx: &Sender<OutboundFrame>, opcode: Opcode, correlation_id: u64, value: &JsonValue) {
    let frames = encode_json_message(opcode, next_message_id(), correlation_id, value);
    for frame in frames {
        let _ = tx.send(OutboundFrame::Body(frame));
    }
}

fn str_field(v: &JsonValue, key: &str) -> String {
    v.get(key)
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

enum ReadOutcome {
    Bytes(usize),
    Idle,
    Closed,
    Error,
}

fn read_into(stream: &mut TcpStream, buf: &mut [u8]) -> ReadOutcome {
    if buf.is_empty() {
        return ReadOutcome::Idle;
    }
    match stream.read(buf) {
        Ok(0) => ReadOutcome::Closed,
        Ok(n) => ReadOutcome::Bytes(n),
        Err(e)
            if e.kind() == ErrorKind::WouldBlock
                || e.kind() == ErrorKind::TimedOut
                || e.kind() == ErrorKind::Interrupted =>
        {
            ReadOutcome::Idle
        }
        Err(_) => ReadOutcome::Error,
    }
}
