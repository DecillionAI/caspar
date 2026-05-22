//! Translation of `chain/hashgraph/roundInfo.go`.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::drivers::network::chain::common::Trilean;
use crate::drivers::network::chain::peers::PeerSet;

/// A round queued for decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PendingRound {
    pub index: i64,
    pub decided: bool,
}

/// Indicates the witness and fame states of an Event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct RoundEvent {
    pub witness: bool,
    pub famous: Trilean,
}

/// Encapsulates information about a round.
///
/// `created_events` is a `BTreeMap` so that marshalling is deterministic,
/// matching the Go code's canonical codec handle.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct RoundInfo {
    /// The events that were "created" in this round.
    pub created_events: BTreeMap<String, RoundEvent>,
    /// The events that were "received" in this round.
    pub received_events: Vec<String>,
    #[serde(skip)]
    pub queued: bool,
    #[serde(skip)]
    pub decided: bool,
}

impl RoundInfo {
    /// Creates a new `RoundInfo`.
    pub fn new() -> RoundInfo {
        RoundInfo {
            created_events: BTreeMap::new(),
            received_events: Vec::new(),
            queued: false,
            decided: false,
        }
    }

    /// Adds an event to the `created_events` map.
    pub fn add_created_event(&mut self, x: &str, witness: bool) {
        self.created_events
            .entry(x.to_string())
            .or_insert(RoundEvent {
                witness,
                famous: Trilean::Undefined,
            });
    }

    /// Adds an event to the `received_events` list.
    pub fn add_received_event(&mut self, x: &str) {
        self.received_events.push(x.to_string());
    }

    /// Sets the famous status of an event.
    pub fn set_fame(&mut self, x: &str, f: bool) {
        let mut e = self
            .created_events
            .get(x)
            .copied()
            .unwrap_or(RoundEvent {
                witness: true,
                famous: Trilean::Undefined,
            });
        e.famous = if f { Trilean::True } else { Trilean::False };
        self.created_events.insert(x.to_string(), e);
    }

    /// Returns true if a super-majority of witnesses are decided and there are
    /// no undecided witnesses. Once a Round is decided it stays decided.
    pub fn witnesses_decided(&mut self, peer_set: &PeerSet) -> bool {
        // if the round was already decided, it stays decided no matter what.
        if self.decided {
            return true;
        }

        let mut c: i64 = 0;
        for e in self.created_events.values() {
            if e.witness && e.famous != Trilean::Undefined {
                c += 1;
            } else if e.witness && e.famous == Trilean::Undefined {
                return false;
            }
        }

        self.decided = c >= peer_set.super_majority();
        self.decided
    }

    /// Returns the round's witnesses.
    pub fn witnesses(&self) -> Vec<String> {
        self.created_events
            .iter()
            .filter(|(_, e)| e.witness)
            .map(|(x, _)| x.clone())
            .collect()
    }

    /// Returns the round's famous witnesses.
    pub fn famous_witnesses(&self) -> Vec<String> {
        self.created_events
            .iter()
            .filter(|(_, e)| e.witness && e.famous == Trilean::True)
            .map(|(x, _)| x.clone())
            .collect()
    }

    /// Returns true unless the famous status of `witness` is undecided.
    pub fn is_decided(&self, witness: &str) -> bool {
        match self.created_events.get(witness) {
            Some(w) => w.witness && w.famous != Trilean::Undefined,
            None => false,
        }
    }

    /// Returns the JSON encoding of a `RoundInfo`.
    pub fn marshal(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Parses a JSON encoded `RoundInfo`.
    pub fn unmarshal(&mut self, data: &[u8]) -> Result<()> {
        *self = serde_json::from_slice(data)?;
        Ok(())
    }

    /// Returns true if the `RoundInfo` is marked as queued.
    pub fn is_queued(&self) -> bool {
        self.queued
    }
}
