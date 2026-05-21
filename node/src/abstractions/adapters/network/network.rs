//! Translation of `abstract/adapters/network/network.go`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::abstractions::adapters::network::chain::IChain;
use crate::abstractions::adapters::network::federation::IFederation;
use crate::abstractions::adapters::network::tcp::ITcp;
use crate::abstractions::adapters::network::ws::IWs;

/// Translation of Go's `crypto/tls.Config` as used by the node.
///
/// Go uses a single `*tls.Config` for both listening and dialing; this struct
/// carries the raw PEM material and the relevant flags so the network driver
/// can build the appropriate `rustls` server/client configuration on demand.
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
