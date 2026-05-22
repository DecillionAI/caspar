//! Translation of `chain/net/commands.go` — the RPC request/response types.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::drivers::network::chain::hashgraph::{Block, Frame, InternalTransaction, WireEvent};
use crate::drivers::network::chain::peers::Peer;

/// The pull part of the pull-push gossip protocol: retrieves unknown Events
/// from another node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SyncRequest {
    #[serde(rename = "FromID")]
    pub from_id: u32,
    pub known: HashMap<u32, i64>,
    pub sync_limit: i64,
    pub work_chain_id: String,
    pub shard_chain_id: String,
}

/// Returns a list of Events as requested by a [`SyncRequest`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SyncResponse {
    #[serde(rename = "FromID")]
    pub from_id: u32,
    pub events: Vec<WireEvent>,
    pub known: HashMap<u32, i64>,
    pub work_chain_id: String,
    pub shard_chain_id: String,
}

/// The push part of the pull-push gossip protocol: actively pushes Events to a
/// node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct EagerSyncRequest {
    #[serde(rename = "FromID")]
    pub from_id: u32,
    pub events: Vec<WireEvent>,
    pub work_chain_id: String,
    pub shard_chain_id: String,
}

/// Indicates the success or failure of an [`EagerSyncRequest`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct EagerSyncResponse {
    #[serde(rename = "FromID")]
    pub from_id: u32,
    pub success: bool,
    pub work_chain_id: String,
    pub shard_chain_id: String,
}

/// Requests a Block, Frame and Snapshot to fast-forward from.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FastForwardRequest {
    #[serde(rename = "FromID")]
    pub from_id: u32,
    pub work_chain_id: String,
    pub shard_chain_id: String,
}

/// Encapsulates the response to a [`FastForwardRequest`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FastForwardResponse {
    #[serde(rename = "FromID")]
    pub from_id: u32,
    pub block: Block,
    pub frame: Frame,
    #[serde(with = "crate::util::bytes_base64")]
    pub snapshot: Vec<u8>,
    pub work_chain_id: String,
    pub shard_chain_id: String,
}

/// Submits an InternalTransaction to join a Babble group.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct JoinRequest {
    pub internal_transaction: InternalTransaction,
    pub work_chain_id: String,
    pub shard_chain_id: String,
}

/// The response to a [`JoinRequest`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct JoinResponse {
    #[serde(rename = "FromID")]
    pub from_id: u32,
    pub accepted: bool,
    pub accepted_round: i64,
    pub peers: Vec<Peer>,
    pub work_chain_id: String,
    pub shard_chain_id: String,
}
