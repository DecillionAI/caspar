//! Per-connection state and the global registry of live container connections.
//!
//! Each docker creature container owns exactly one [`GatewayConnection`]. The
//! owning `TcpStream` lives inside that connection's dedicated I/O thread (see
//! [`super::server`]); everything reachable through `GatewayConnection` is safe
//! to call from any thread. Outbound frames are handed to the I/O thread over
//! an MPSC channel so signal pushes never block on socket writes and never
//! contend with the inbound read loop.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;

use crate::drivers::vmm::network::docker_host::protocol::{
    encode_json_message, next_message_id, Opcode,
};
use crate::drivers::vmm::prelude::*;

/// Frames the connection I/O thread accepts from any thread.
pub(crate) enum OutboundFrame {
    Body(Vec<u8>),
    Shutdown,
}

/// The identity a container declares during the `HELLO` handshake. Mirrors the
/// VM execution context the node already tracks (`register_vm_context`).
#[derive(Clone, Debug, Default)]
pub(crate) struct ContainerIdentity {
    pub(crate) vm_id: String,
    pub(crate) machine_id: String,
    pub(crate) creature_id: String,
    pub(crate) program_id: String,
}

/// A single live container connection.
pub(crate) struct GatewayConnection {
    pub(crate) conn_id: u64,
    pub(crate) peer: String,
    pub(crate) identity: ContainerIdentity,
    disconnected: AtomicBool,
    outbound: Mutex<Option<Sender<OutboundFrame>>>,
}

impl GatewayConnection {
    pub(crate) fn new(
        conn_id: u64,
        peer: String,
        identity: ContainerIdentity,
        outbound: Sender<OutboundFrame>,
    ) -> Arc<GatewayConnection> {
        Arc::new(GatewayConnection {
            conn_id,
            peer,
            identity,
            disconnected: AtomicBool::new(false),
            outbound: Mutex::new(Some(outbound)),
        })
    }

    /// Queue an already-encoded frame for the I/O thread. Silent no-op once the
    /// connection has been torn down.
    pub(crate) fn enqueue(&self, frame: Vec<u8>) {
        if self.disconnected.load(Ordering::Acquire) {
            return;
        }
        if let Some(tx) = self.outbound.lock().unwrap().as_ref() {
            let _ = tx.send(OutboundFrame::Body(frame));
        }
    }

    /// Enqueue every frame of an encoded (possibly multi-chunk) message.
    pub(crate) fn enqueue_all(&self, frames: Vec<Vec<u8>>) {
        for frame in frames {
            self.enqueue(frame);
        }
    }

    /// Push a signal/notification to the container. The payload is delivered to
    /// the container's signal handler as `{key, data}`.
    pub(crate) fn push_signal(&self, key: &str, data: &JsonValue) {
        let envelope = json!({ "key": key, "data": data });
        let frames = encode_json_message(Opcode::Signal, next_message_id(), 0, &envelope);
        self.enqueue_all(frames);
    }

    /// Idempotent cooperative shutdown — wakes the I/O thread so it can flush
    /// and close.
    pub(crate) fn shutdown(&self) {
        if self.disconnected.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(tx) = self.outbound.lock().unwrap().take() {
            let _ = tx.send(OutboundFrame::Shutdown);
        }
    }

    pub(crate) fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }
}

/// Process-wide registry of live container connections.
///
/// Lookups iterate the (small) live-connection set rather than maintaining
/// secondary indexes — there is one connection per running docker VM, so the
/// linear scan is cheap and keeps the registry free of index-consistency bugs.
pub(crate) struct GatewayRegistry {
    conns: DashMap<u64, Arc<GatewayConnection>>,
    next_id: AtomicU64,
}

impl GatewayRegistry {
    fn new() -> Self {
        Self {
            conns: DashMap::new(),
            next_id: AtomicU64::new(1),
        }
    }

    pub(crate) fn alloc_conn_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn insert(&self, conn: Arc<GatewayConnection>) {
        // A re-connecting VM (same vm_id) supersedes any stale connection so
        // signals are never delivered to a dead socket.
        let vm_id = conn.identity.vm_id.clone();
        if !vm_id.is_empty() {
            let stale: Vec<u64> = self
                .conns
                .iter()
                .filter(|e| e.value().identity.vm_id == vm_id && e.value().conn_id != conn.conn_id)
                .map(|e| *e.key())
                .collect();
            for id in stale {
                if let Some((_, old)) = self.conns.remove(&id) {
                    old.shutdown();
                }
            }
        }
        self.conns.insert(conn.conn_id, conn);
    }

    pub(crate) fn remove(&self, conn_id: u64) {
        self.conns.remove(&conn_id);
    }

    pub(crate) fn count(&self) -> usize {
        self.conns.len()
    }

    /// Resolve a connection by its declared `vm_id` (the precise target).
    pub(crate) fn by_vm(&self, vm_id: &str) -> Option<Arc<GatewayConnection>> {
        if vm_id.is_empty() {
            return None;
        }
        self.conns
            .iter()
            .find(|e| e.value().identity.vm_id == vm_id && !e.value().is_disconnected())
            .map(|e| e.value().clone())
    }

    /// All live connections belonging to a machine creature.
    pub(crate) fn by_machine(&self, machine_id: &str) -> Vec<Arc<GatewayConnection>> {
        if machine_id.is_empty() {
            return Vec::new();
        }
        self.conns
            .iter()
            .filter(|e| e.value().identity.machine_id == machine_id && !e.value().is_disconnected())
            .map(|e| e.value().clone())
            .collect()
    }

    /// Push a signal to every live container of `machine_id`. Returns the number
    /// of containers the signal was delivered to.
    pub(crate) fn push_signal_to_machine(&self, machine_id: &str, key: &str, data: &JsonValue) -> usize {
        let conns = self.by_machine(machine_id);
        for conn in &conns {
            conn.push_signal(key, data);
        }
        conns.len()
    }
}

/// The single process-wide gateway connection registry.
pub(crate) static GATEWAY_REGISTRY: Lazy<GatewayRegistry> = Lazy::new(GatewayRegistry::new);
