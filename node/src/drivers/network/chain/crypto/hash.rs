//! Translation of `chain/crypto/hash.go`.

use sha2::{Digest, Sha256};

/// Returns the SHA256 hash of the data.
#[allow(non_snake_case)]
pub fn sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

/// Returns the SHA256 hash of the concatenation of `left` and `right`.
pub fn simple_hash_from_two_hashes(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().to_vec()
}
