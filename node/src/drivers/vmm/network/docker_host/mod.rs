//! Docker-host bridge gateway.
//!
//! This is the unified ingress/egress for **docker-based creature programs**.
//! Such containers run sandboxed on the gateway docker network with no other
//! route to the outside world; their *only* channel is a single TCP connection
//! to the gateway served here. Over that one connection a container:
//!
//! * performs every host interaction — DB ops, storage ops, outbound HTTP,
//!   signalling other creatures, spawning sibling VMs — by issuing host-call
//!   `REQUEST`s that the node executes through its VM host functions, and
//! * receives `SIGNAL` pushes when other creatures signal it.
//!
//! The module is structured into:
//! * [`protocol`]   — the chunked wire framing,
//! * [`connection`] — per-connection state and the live-connection registry,
//! * [`dispatch`]   — mapping requests onto the unified host-function surface,
//! * [`server`]     — the TCP listener and per-connection I/O loop.

pub(crate) mod connection;
pub(crate) mod dispatch;
pub(crate) mod protocol;
pub(crate) mod server;

use crate::drivers::vmm::prelude::*;

use crate::drivers::vmm::network::docker_host::connection::GATEWAY_REGISTRY;

/// Start the gateway TCP listener (no-op when `port <= 0`).
pub fn start(port: i64) {
    server::start(port);
}

/// Push a signal to every live docker container belonging to `machine_id`.
///
/// Returns the number of containers the signal reached. A return of `0` means
/// no docker creature of that machine is currently connected — callers use this
/// to decide whether to fall back to the spawn-on-signal path.
pub fn push_signal_to_machine(machine_id: &str, key: &str, data: &JsonValue) -> usize {
    GATEWAY_REGISTRY.push_signal_to_machine(machine_id, key, data)
}

/// Push a signal to the specific container identified by `vm_id`. Returns `true`
/// if a live container received it.
pub fn push_signal_to_vm(vm_id: &str, key: &str, data: &JsonValue) -> bool {
    match GATEWAY_REGISTRY.by_vm(vm_id) {
        Some(conn) => {
            conn.push_signal(key, data);
            true
        }
        None => false,
    }
}

/// Number of docker containers currently connected to the gateway.
pub fn live_connection_count() -> usize {
    GATEWAY_REGISTRY.count()
}
