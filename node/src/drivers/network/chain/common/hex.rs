//! Translation of `chain/common/hex.go`.

/// Returns the UPPERCASE hex string representation of `hex_bytes` with the `0X`
/// prefix.
pub fn encode_to_string(hex_bytes: &[u8]) -> String {
    format!("0X{}", ::hex::encode_upper(hex_bytes))
}

/// Converts a hex string with the `0X` prefix to a byte slice.
pub fn decode_from_string(hex_string: &str) -> anyhow::Result<Vec<u8>> {
    Ok(::hex::decode(&hex_string[2..])?)
}
