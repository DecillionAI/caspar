//! Translation of `abstract/adapters/network/tcp.go`.

use crate::abstractions::adapters::network::network::TlsConfig;

/// The raw TCP client API driver interface.
pub trait ITcp: Send + Sync {
    fn listen(&self, port: i64, tls_config: Option<TlsConfig>);
}
