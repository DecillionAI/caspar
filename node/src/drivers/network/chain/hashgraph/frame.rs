//! Translation of `chain/hashgraph/frame.go`.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::event::{sort_frame_events, FrameEvent};
use super::root::Root;
use crate::drivers::network::chain::crypto;
use crate::drivers::network::chain::peers::Peer;

/// Represents a section of the hashgraph.
///
/// The maps use `BTreeMap` so that marshalling is deterministic, matching the
/// Go code's use of `codec.JsonHandle{Canonical: true}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Frame {
    /// RoundReceived.
    pub round: i64,
    /// The authoritative peer-set at Round.
    pub peers: Vec<Peer>,
    /// Roots on top of which Frame Events can be inserted.
    pub roots: BTreeMap<String, Root>,
    /// Events with RoundReceived = Round.
    pub events: Vec<FrameEvent>,
    /// Full peer-set history (`[round] => Peers`).
    pub peer_sets: BTreeMap<i64, Vec<Peer>>,
    /// Unix timestamp (median of round-received famous witnesses).
    pub timestamp: i64,
}

impl Frame {
    /// Returns all the events in the Frame, including events in roots, sorted
    /// by Lamport timestamp.
    pub fn sorted_frame_events(&self) -> Vec<FrameEvent> {
        let mut sorted: Vec<FrameEvent> = Vec::new();
        for r in self.roots.values() {
            sorted.extend(r.events.iter().cloned());
        }
        sorted.extend(self.events.iter().cloned());
        sort_frame_events(&mut sorted);
        sorted
    }

    /// Returns the JSON encoding of the Frame.
    pub fn marshal(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Parses a JSON encoded Frame.
    pub fn unmarshal(&mut self, data: &[u8]) -> Result<()> {
        *self = serde_json::from_slice(data)?;
        Ok(())
    }

    /// Returns the SHA256 hash of the marshalled Frame.
    pub fn hash(&self) -> Result<Vec<u8>> {
        Ok(crypto::sha256(&self.marshal()?))
    }
}
