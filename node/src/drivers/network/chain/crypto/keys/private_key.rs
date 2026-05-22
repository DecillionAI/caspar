//! Translation of `chain/crypto/keys/private_key.go`.
//!
//! Go represented keys as `*ecdsa.PrivateKey`; this translation uses
//! `k256::ecdsa::SigningKey`. The `paddedBigBytes`/`readBits` helpers from the
//! Go file produced a fixed-width big-endian encoding of the key's D value —
//! `SigningKey::to_bytes()` already returns exactly that 32-byte form, so those
//! private helpers have no separate translation.

use anyhow::{anyhow, Result};
use k256::ecdsa::SigningKey;
use k256::FieldBytes;

/// Creates a new secp256k1 `SigningKey`.
///
/// Mirrors `ecdsa.GenerateKey`: random 32-byte scalars are drawn until one is a
/// valid private key (non-zero and below the curve order N).
pub fn generate_ecdsa_key() -> Result<SigningKey> {
    loop {
        let mut d = [0u8; 32];
        getrandom::getrandom(&mut d).map_err(|e| anyhow!("getrandom failed: {}", e))?;
        if let Ok(key) = SigningKey::from_bytes(FieldBytes::from_slice(&d)) {
            return Ok(key);
        }
    }
}

/// Exports a private key into a binary dump (32-byte big-endian D value).
pub fn dump_private_key(priv_key: &SigningKey) -> Vec<u8> {
    priv_key.to_bytes().to_vec()
}

/// Creates a private key with the given D value.
pub fn parse_private_key(d: &[u8]) -> Result<SigningKey> {
    if 8 * d.len() != 256 {
        return Err(anyhow!("invalid length, need 256 bits"));
    }
    SigningKey::from_bytes(FieldBytes::from_slice(d)).map_err(|_| {
        // Distinguish the two validation failures Go reported explicitly.
        anyhow!("invalid private key, zero or >=N")
    })
}

/// Returns the hexadecimal representation of a raw private key.
pub fn private_key_hex(key: &SigningKey) -> String {
    hex::encode(dump_private_key(key))
}
