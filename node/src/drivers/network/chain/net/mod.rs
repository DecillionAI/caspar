//! Translation of `chain/net` — the transports Babble nodes use to exchange
//! RPC requests (SyncRequest, EagerSyncRequest, JoinRequest, ...).
//!
//! This part translates the transport-independent interface layer: the RPC
//! command/response types, the `RPC` envelope, and the `Transport` trait. The
//! concrete transports (in-memory, TCP, WebRTC) and the WAMP signalling client
//! are translated in later parts.

pub mod commands;
pub mod inmem_transport;
pub mod rpc;
pub mod transport;

pub use commands::{
    EagerSyncRequest, EagerSyncResponse, FastForwardRequest, FastForwardResponse, JoinRequest,
    JoinResponse, SyncRequest, SyncResponse,
};
pub use inmem_transport::InmemTransport;
pub use rpc::{Rpc, RpcCommand, RpcResponse, RpcResponseKind};
pub use transport::Transport;
