//! Translation of `drivers/network/federation` — the inter-organisation
//! TCP RPC the Caspar shell uses to talk to peer nodes (sister Caspar
//! deployments) and federate updates / requests / responses across them.
//!
//! Modules:
//!   * `netserver` — the framed TCP server (`Tcp` / `Socket`).
//!   * `federation` — the `IFederation` implementation wired on top.

pub mod federation;
pub mod netserver;

pub use federation::FedNet;
pub use netserver::{Socket, Tcp};
