//! Translation of `chain/peers/peer.go`.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::drivers::network::chain::common;
use crate::drivers::network::chain::crypto::keys;

/// Holds Peer data.
///
/// The Go struct cached its `id` in an unexported field; the translation
/// computes [`Peer::id`] on demand instead, which keeps `Peer` cheaply
/// cloneable and thread-safe. The cache was invisible to all callers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Peer {
    /// The IP:PORT of the Babble node. Not necessary with WebRTC.
    pub net_addr: String,
    /// The hexadecimal representation of the peer's public key.
    pub pub_key_hex: String,
    /// An optional, non-unique friendly name for the peer.
    pub moniker: String,
}

impl Peer {
    /// Instantiates a Peer.
    pub fn new(pub_key_hex: &str, net_addr: &str, moniker: &str) -> Peer {
        Peer {
            pub_key_hex: pub_key_hex.to_string(),
            net_addr: net_addr.to_string(),
            moniker: moniker.to_string(),
        }
    }

    /// An ID for the peer, calculated from the public key.
    pub fn id(&self) -> u32 {
        keys::public_key_id(&self.pub_key_bytes())
    }

    /// The upper-case version of `pub_key_hex`, used for indexing in maps.
    pub fn pub_key_string(&self) -> String {
        self.pub_key_hex.to_uppercase()
    }

    /// The byte slice representation of the Peer's public key.
    pub fn pub_key_bytes(&self) -> Vec<u8> {
        common::decode_from_string(&self.pub_key_hex).unwrap_or_default()
    }

    /// Marshals the Peer object. As in Go, the (computed) id is excluded.
    pub fn marshal(&self) -> Result<Vec<u8>> {
        // Go's `json.Encoder.Encode` appends a trailing newline.
        let mut b = serde_json::to_vec(self)?;
        b.push(b'\n');
        Ok(b)
    }

    /// Unmarshals a byte slice into this Peer object.
    pub fn unmarshal(&mut self, data: &[u8]) -> Result<()> {
        *self = serde_json::from_slice(data)?;
        Ok(())
    }
}

/// Excludes a single peer (by id) from a list of peers. Returns the index of
/// the excluded peer (or -1) and the remaining peers.
pub fn exclude_peer(peers: &[Peer], peer: u32) -> (i64, Vec<Peer>) {
    let mut index: i64 = -1;
    let mut other_peers: Vec<Peer> = Vec::with_capacity(peers.len());
    for (i, p) in peers.iter().enumerate() {
        if p.id() != peer {
            other_peers.push(p.clone());
        } else {
            index = i as i64;
        }
    }
    (index, other_peers)
}
