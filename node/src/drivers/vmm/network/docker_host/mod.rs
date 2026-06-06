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
//! ## Ownership & access
//!
//! The gateway is **not** a process-wide static. A single [`DockerHostGateway`]
//! is owned by the `Vmm` driver and reached through the canonical
//! `ICore → tools() → vmm()` object graph, like the rest of the VMM subsystem.
//!
//! ## Security
//!
//! Containers authenticate with a node-issued session token; the node resolves
//! it to the VM's authoritative identity. A container never declares its own
//! `vmId`/`creatureId`/`programId`, so it cannot impersonate another VM.
//!
//! The module is structured into:
//! * [`protocol`]   — the chunked wire framing,
//! * [`connection`] — per-connection state and the live-connection registry,
//! * [`dispatch`]   — mapping requests onto the unified host-function surface,
//! * [`gateway`]    — the owning `DockerHostGateway` instance,
//! * [`server`]     — the per-connection I/O loop.

pub(crate) mod connection;
pub(crate) mod dispatch;
pub(crate) mod gateway;
pub(crate) mod protocol;
pub(crate) mod server;

pub(crate) use connection::ContainerIdentity;
pub(crate) use gateway::DockerHostGateway;
