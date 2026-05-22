//! Translation of `chain/peers/peer_set.go`.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::Result;

use super::peer::Peer;
use crate::drivers::network::chain::common;
use crate::drivers::network::chain::crypto;

/// Represents a collection of peers.
///
/// Go stored peers as `*Peer` shared between the slice and the two maps; since
/// `Peer` is an immutable value type here, the maps hold clones. The lazily
/// computed caches use `OnceLock` instead of mutable cache fields.
#[derive(Debug)]
pub struct PeerSet {
    pub peers: Vec<Peer>,
    pub by_pub_key: HashMap<String, Peer>,
    pub by_id: HashMap<u32, Peer>,

    // cached values
    hash: OnceLock<Vec<u8>>,
    hex: OnceLock<String>,
    super_majority: OnceLock<i64>,
    trust_count: OnceLock<i64>,
}

impl PeerSet {
    /// Creates a new `PeerSet` from a list of Peers.
    pub fn new(peers: Vec<Peer>) -> PeerSet {
        let mut peer_set = PeerSet {
            peers,
            by_pub_key: HashMap::new(),
            by_id: HashMap::new(),
            hash: OnceLock::new(),
            hex: OnceLock::new(),
            super_majority: OnceLock::new(),
            trust_count: OnceLock::new(),
        };
        peer_set.init_maps();
        peer_set
    }

    fn init_maps(&mut self) {
        self.by_pub_key.clear();
        self.by_id.clear();
        for peer in &self.peers {
            self.by_pub_key.insert(peer.pub_key_string(), peer.clone());
            self.by_id.insert(peer.id(), peer.clone());
        }
    }

    /// Returns a new `PeerSet` including the new peer.
    pub fn with_new_peer(&self, peer: &Peer) -> PeerSet {
        let mut peers = self.peers.clone();
        // don't add it if it already exists
        if !self.by_id.contains_key(&peer.id()) {
            peers.push(peer.clone());
        }
        PeerSet::new(peers)
    }

    /// Returns a new `PeerSet` excluding the provided peer.
    pub fn with_removed_peer(&self, peer: &Peer) -> PeerSet {
        let peers: Vec<Peer> = self
            .peers
            .iter()
            .filter(|p| p.pub_key_hex != peer.pub_key_hex)
            .cloned()
            .collect();
        PeerSet::new(peers)
    }

    /// Returns the PeerSet's slice of public keys.
    pub fn pub_keys(&self) -> Vec<String> {
        self.peers.iter().map(|p| p.pub_key_string()).collect()
    }

    /// Returns the PeerSet's slice of IDs.
    pub fn ids(&self) -> Vec<u32> {
        self.peers.iter().map(|p| p.id()).collect()
    }

    /// Returns the number of Peers in the PeerSet.
    pub fn len(&self) -> i64 {
        self.by_pub_key.len() as i64
    }

    pub fn is_empty(&self) -> bool {
        self.by_pub_key.is_empty()
    }

    /// Uniquely identifies a PeerSet, by hashing their public keys together.
    pub fn hash(&self) -> Vec<u8> {
        self.hash
            .get_or_init(|| {
                let mut hash: Vec<u8> = Vec::new();
                for p in &self.peers {
                    let pk = p.pub_key_bytes();
                    hash = crypto::simple_hash_from_two_hashes(&hash, &pk);
                }
                hash
            })
            .clone()
    }

    /// The hexadecimal representation of [`PeerSet::hash`].
    pub fn hex(&self) -> String {
        self.hex
            .get_or_init(|| common::encode_to_string(&self.hash()))
            .clone()
    }

    /// Marshals the PeerSet (its peer slice).
    pub fn marshal(&self) -> Result<Vec<u8>> {
        // Go's `json.Encoder.Encode` appends a trailing newline.
        let mut buf = serde_json::to_vec(&self.peers)?;
        buf.push(b'\n');
        Ok(buf)
    }

    /// Unmarshals a PeerSet. Go's `Unmarshal` mutated the receiver; the
    /// translation is an associated constructor.
    pub fn unmarshal(peer_slice_bytes: &[u8]) -> Result<PeerSet> {
        let peers: Vec<Peer> = serde_json::from_slice(peer_slice_bytes)?;
        Ok(PeerSet::new(peers))
    }

    /// The number of peers that forms a strong majority (+2/3) in the PeerSet.
    pub fn super_majority(&self) -> i64 {
        *self.super_majority.get_or_init(|| 2 * self.len() / 3 + 1)
    }

    /// The minimum number of signatures that may represent the finality of a
    /// state, given the assumptions of the consensus algorithm.
    pub fn trust_count(&self) -> i64 {
        *self.trust_count.get_or_init(|| {
            if self.peers.len() > 1 {
                (self.len() as f64 / 3.0).ceil() as i64
            } else {
                0
            }
        })
    }

    /// Clears the cached `hash`, `hex` and `super_majority` values. As in Go,
    /// `trust_count` is intentionally left untouched.
    pub fn clear_cache(&mut self) {
        self.hash = OnceLock::new();
        self.hex = OnceLock::new();
        self.super_majority = OnceLock::new();
    }
}
