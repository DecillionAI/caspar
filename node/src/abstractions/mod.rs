//! Hexagonal-architecture boundary layer.
//!
//! - `ports` — driver/adapter trait interfaces (what the core needs from the
//!   outside world: storage, security, signaler, vmm, file, network).
//! - `models` — shared domain models exchanged across `core`, `ports`, and
//!   `shell` (actions, packets, transactions, chain/globe state).
//! - `state` — actor lifecycle state machine.

pub mod models;
pub mod ports;
pub mod state;
