//! Translation of `chain/hashgraph/root.go`.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::event::FrameEvent;
use crate::drivers::network::chain::common;
use crate::drivers::network::chain::crypto;

/// Forms a base on top of which a participant's Events can be inserted. It
/// contains `FrameEvent`s sorted by Lamport timestamp.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Root {
    pub events: Vec<FrameEvent>,
}

impl Root {
    /// Instantiates a new empty root.
    pub fn new() -> Root {
        Root { events: Vec::new() }
    }

    /// Appends a `FrameEvent` to the root's event slice. Items are assumed to
    /// be inserted in topological order.
    pub fn insert(&mut self, frame_event: FrameEvent) {
        self.events.push(frame_event);
    }

    /// Returns the JSON encoding of a Root.
    pub fn marshal(&self) -> Result<Vec<u8>> {
        // Go uses `json.Encoder`, which appends a trailing newline.
        let mut b = serde_json::to_vec(self)?;
        b.push(b'\n');
        Ok(b)
    }

    /// Parses a JSON encoded Root.
    pub fn unmarshal(&mut self, data: &[u8]) -> Result<()> {
        *self = serde_json::from_slice(data)?;
        Ok(())
    }

    /// Returns the SHA256 hash of the marshalled Root.
    pub fn hash(&self) -> Result<String> {
        let hash_bytes = self.marshal()?;
        let hash = crypto::sha256(&hash_bytes);
        Ok(common::encode_to_string(&hash))
    }
}
