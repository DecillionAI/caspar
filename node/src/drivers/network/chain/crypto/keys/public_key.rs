//! Translation of `chain/crypto/keys/public_key.go`.

use k256::ecdsa::VerifyingKey;
use k256::elliptic_curve::sec1::ToEncodedPoint;

use crate::drivers::network::chain::common;

/// Parses the uncompressed form of a public key (as produced by
/// [`from_public_key`]). Returns `None` for an empty or invalid input, matching
/// Go's `elliptic.Unmarshal` returning nil coordinates.
pub fn to_public_key(pub_bytes: &[u8]) -> Option<VerifyingKey> {
    if pub_bytes.is_empty() {
        return None;
    }
    VerifyingKey::from_sec1_bytes(pub_bytes).ok()
}

/// Outputs the public key in uncompressed form (65 bytes: `0x04 || X || Y`).
pub fn from_public_key(pub_key: &VerifyingKey) -> Vec<u8> {
    pub_key.to_encoded_point(false).as_bytes().to_vec()
}

/// Tries to give a unique `u32` representation of the public key. Used to save
/// space in the wire encoding of hashgraph Events.
pub fn public_key_id(pub_bytes: &[u8]) -> u32 {
    hash32(pub_bytes)
}

/// The 32-bit FNV-1a hash of `data`.
fn hash32(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5; // FNV offset basis
    for &b in data {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193); // FNV prime
    }
    h
}

/// Returns the hexadecimal representation of the uncompressed public key.
pub fn public_key_hex(pub_key: &VerifyingKey) -> String {
    common::encode_to_string(&from_public_key(pub_key))
}
