//! Network port traits — the four transports plus the top-level
//! [`INetwork`] facade that bundles them.
//!
//! - `chain` — consensus chain transport (`IChain`)
//! - `federation` — federation transport (`IFederation`)
//! - `tcp`, `ws` — client transports (`ITcp`, `IWs`)
//!
//! The top-level [`INetwork`] trait and the shared [`TlsConfig`] live in
//! this module file rather than a `network::network` submodule.

use std::collections::HashMap;
use std::sync::Arc;

pub mod chain;
pub mod federation;
pub mod tcp;
pub mod ws;

pub use chain::IChain;
pub use federation::IFederation;
pub use tcp::ITcp;
pub use ws::IWs;

/// Translation of Go's `crypto/tls.Config` as used by the node.
///
/// Go uses a single `*tls.Config` for both listening and dialing; this
/// struct carries the raw PEM material and the relevant flags so the
/// network driver can build the appropriate `rustls` server/client
/// configuration on demand.
#[derive(Clone, Default)]
pub struct TlsConfig {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    pub ca_pem: Vec<u8>,
    pub insecure_skip_verify: bool,
    pub server_name: String,
}

/// The top-level network driver interface.
pub trait INetwork: Send + Sync {
    fn chain(&self) -> Arc<dyn IChain>;
    fn federation(&self) -> Arc<dyn IFederation>;
    fn tcp(&self) -> Arc<dyn ITcp>;
    fn ws(&self) -> Arc<dyn IWs>;
    fn tls_config(&self) -> Option<TlsConfig>;
    fn run(&self, ports: HashMap<String, i64>);
}
