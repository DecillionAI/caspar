//! Translation of `chain/hashgraph/internal_transaction.go`.

use anyhow::Result;
use k256::ecdsa::SigningKey;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

use crate::drivers::network::chain::crypto;
use crate::drivers::network::chain::crypto::keys;
use crate::drivers::network::chain::peers::Peer;

/// Denotes the nature of an [`InternalTransaction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
#[repr(u8)]
pub enum TransactionType {
    /// Add a peer.
    PeerAdd = 0,
    /// Remove a peer.
    PeerRemove = 1,
}

impl Default for TransactionType {
    fn default() -> Self {
        TransactionType::PeerAdd
    }
}

impl std::fmt::Display for TransactionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TransactionType::PeerAdd => "PEER_ADD",
            TransactionType::PeerRemove => "PEER_REMOVE",
        };
        write!(f, "{}", s)
    }
}

/// Contains the payload of an [`InternalTransaction`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct InternalTransactionBody {
    /// Add or Remove.
    #[serde(rename = "Type")]
    pub typ: TransactionType,
    /// Targeted Peer.
    pub peer: Peer,
}

impl InternalTransactionBody {
    /// Returns the JSON encoding of an `InternalTransactionBody`.
    pub fn marshal(&self) -> Result<Vec<u8>> {
        let mut b = serde_json::to_vec(self)?;
        b.push(b'\n');
        Ok(b)
    }

    /// Returns the SHA256 hash of the `InternalTransactionBody`.
    pub fn hash(&self) -> Result<Vec<u8>> {
        Ok(crypto::sha256(&self.marshal()?))
    }
}

/// A special transaction interpreted by Babble to act on its own internal
/// state (adding or removing validators). InternalTransactions also go through
/// consensus.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct InternalTransaction {
    pub body: InternalTransactionBody,
    pub signature: String,
}

impl InternalTransaction {
    /// Creates a new `InternalTransaction`.
    pub fn new(t_type: TransactionType, peer: Peer) -> InternalTransaction {
        InternalTransaction {
            body: InternalTransactionBody { typ: t_type, peer },
            signature: String::new(),
        }
    }

    /// Creates a new `InternalTransaction` to add a peer.
    pub fn new_join(peer: Peer) -> InternalTransaction {
        InternalTransaction::new(TransactionType::PeerAdd, peer)
    }

    /// Creates a new `InternalTransaction` to remove a peer.
    pub fn new_leave(peer: Peer) -> InternalTransaction {
        InternalTransaction::new(TransactionType::PeerRemove, peer)
    }

    /// Returns the JSON encoding of an `InternalTransaction`.
    pub fn marshal(&self) -> Result<Vec<u8>> {
        let mut b = serde_json::to_vec(self)?;
        b.push(b'\n');
        Ok(b)
    }

    /// Parses an `InternalTransaction` from JSON.
    pub fn unmarshal(&mut self, data: &[u8]) -> Result<()> {
        *self = serde_json::from_slice(data)?;
        Ok(())
    }

    /// Signs the SHA256 hash of the transaction's body.
    pub fn sign(&mut self, priv_key: &SigningKey) -> Result<()> {
        let sign_bytes = self.body.hash()?;
        let (r, s) = keys::sign(priv_key, &sign_bytes)?;
        self.signature = keys::encode_signature(&r, &s);
        Ok(())
    }

    /// Verifies the transaction's signature.
    pub fn verify(&self) -> Result<bool> {
        let pub_bytes = self.body.peer.pub_key_bytes();
        let pub_key = match keys::to_public_key(&pub_bytes) {
            Some(pk) => pk,
            None => return Ok(false),
        };

        let sign_bytes = self.body.hash()?;
        let (r, s) = keys::decode_signature(&self.signature)?;

        Ok(keys::verify(&pub_key, &sign_bytes, &r, &s))
    }

    /// A string representation of the body's hash, used as a map key while an
    /// `InternalTransaction` goes through consensus.
    ///
    /// Go returned the raw hash bytes as a Go string; Rust strings must be
    /// valid UTF-8, so the translation returns the hex-encoded hash instead —
    /// an equally injective key.
    pub fn hash_string(&self) -> String {
        match self.body.hash() {
            Ok(h) => hex::encode(h),
            Err(_) => String::new(),
        }
    }

    /// Returns a receipt accepting this `InternalTransaction`.
    pub fn as_accepted(&self) -> InternalTransactionReceipt {
        InternalTransactionReceipt {
            internal_transaction: self.clone(),
            accepted: true,
        }
    }

    /// Returns a receipt refusing this `InternalTransaction`.
    pub fn as_refused(&self) -> InternalTransactionReceipt {
        InternalTransactionReceipt {
            internal_transaction: self.clone(),
            accepted: false,
        }
    }
}

/// Records the decision by the application to accept or refuse an
/// [`InternalTransaction`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct InternalTransactionReceipt {
    pub internal_transaction: InternalTransaction,
    pub accepted: bool,
}
