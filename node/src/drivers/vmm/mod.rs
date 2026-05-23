//! Translation of `drivers/vmm` — the virtual-machine driver.
//!
//! The driver speaks to the Caspar app-engine (a separate process) over two
//! ZeroMQ sockets: a REP socket (`tcp://*:5555`) for host calls coming from
//! the engine, and a REQ socket (`tcp://localhost:5556`) for VM control
//! commands going *to* the engine. Inbound host calls are decoded as JSON,
//! dispatched through [`Vmm::vm_callback`], and the result is queued back
//! onto the outbound REQ channel as an `apiResponse`.

pub mod hostcall_entities;
pub mod hostcall_global;
pub mod hostcall_logs;
pub mod appengine;
pub mod vmm;

pub use vmm::Vmm;
