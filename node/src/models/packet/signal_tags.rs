//! Signal tags — the sender-supplied labels stored alongside a persisted
//! signal packet, and the query that filters the store log by them.
//!
//! A tag is a short `key=value` (or bare) label the *sender* attaches to a
//! signal. The node stores them with the packet in the time-series log so a
//! store's signals can later be filtered richly — by conversation thread, by
//! message kind, by the agent that produced it — without the sender having to
//! keep a parallel index anywhere else.
//!
//! Encoding: tags are stored in one column as `|a=1|b=2|`, always with a
//! leading and trailing separator, so a whole-tag match is the substring
//! `|<tag>|` and can never match a prefix of a longer tag. Tags are validated,
//! never escaped: a tag carrying a separator, a quote, or a SQL `LIKE`
//! wildcard is rejected outright, so a malformed tag fails its signal instead
//! of silently widening somebody else's filter.

use anyhow::{anyhow, Result};

/// Separator framing every tag in the stored column.
pub const TAG_SEP: char = '|';
/// Longest single tag accepted, in bytes.
pub const MAX_TAG_LEN: usize = 128;
/// Most tags accepted on one signal.
pub const MAX_TAGS: usize = 24;

/// True when `c` may appear in a tag. Deliberately narrow: alphanumerics plus
/// the punctuation ids and keys actually use (`1@global`, `kind=message`,
/// `thread=main`, `run=a1b2`). `|`, `'`, `%` and `_` are excluded because they
/// are the column separator, the SQL string delimiter, and the two `LIKE`
/// wildcards.
fn is_tag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '=' | '@' | '.' | ':' | '-' | '/' | '+' | '#')
}

/// Validate one tag, returning it trimmed. Errors name the offending tag so a
/// rejected signal is diagnosable from the response alone.
pub fn validate_tag(tag: &str) -> Result<String> {
    let t = tag.trim();
    if t.is_empty() {
        return Err(anyhow!("empty tag"));
    }
    if t.len() > MAX_TAG_LEN {
        return Err(anyhow!("tag longer than {} bytes: {}", MAX_TAG_LEN, t));
    }
    if let Some(bad) = t.chars().find(|c| !is_tag_char(*c)) {
        return Err(anyhow!("tag contains unsupported character {:?}: {}", bad, t));
    }
    Ok(t.to_string())
}

/// Validate a whole tag list, de-duplicating while preserving order.
pub fn validate_tags(tags: &[String]) -> Result<Vec<String>> {
    if tags.len() > MAX_TAGS {
        return Err(anyhow!("more than {} tags on one signal", MAX_TAGS));
    }
    let mut out: Vec<String> = Vec::with_capacity(tags.len());
    for raw in tags {
        let t = validate_tag(raw)?;
        if !out.contains(&t) {
            out.push(t);
        }
    }
    Ok(out)
}

/// Encode validated tags into the stored column form, `|a|b|`. An empty list
/// encodes as the empty string so "has no tags" stays distinguishable.
pub fn encode_tags(tags: &[String]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    let mut s = String::with_capacity(tags.iter().map(|t| t.len() + 1).sum::<usize>() + 1);
    s.push(TAG_SEP);
    for t in tags {
        s.push_str(t);
        s.push(TAG_SEP);
    }
    s
}

/// Decode a stored tag column back into a list.
pub fn decode_tags(encoded: &str) -> Vec<String> {
    encoded
        .split(TAG_SEP)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// A filtered read over one store's persisted signals.
///
/// `tags_all` and `tags_any` are ANDed with each other (`all` tags must be
/// present AND at least one of `any`), which covers the real queries: "this
/// thread AND (a human message OR an agent answer)".
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Every one of these tags must be on the packet.
    pub tags_all: Vec<String>,
    /// At least one of these tags must be on the packet (empty = no constraint).
    pub tags_any: Vec<String>,
    /// Strictly-before cursor on `time` (0 = no bound). Pages backwards.
    pub before_time: i64,
    /// Strictly-after bound on `time` (0 = no bound).
    pub after_time: i64,
    /// Row cap. Clamped by the driver.
    pub count: i64,
}

impl LogQuery {
    /// Validate every tag in the query the same way a sender's tags are
    /// validated, so a query can never inject into the generated `LIKE`.
    pub fn validated(mut self) -> Result<LogQuery> {
        self.tags_all = validate_tags(&self.tags_all)?;
        self.tags_any = validate_tags(&self.tags_any)?;
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_tags() {
        let tags = vec!["kind=message".to_string(), "thread=main".to_string()];
        let encoded = encode_tags(&tags);
        assert_eq!(encoded, "|kind=message|thread=main|");
        assert_eq!(decode_tags(&encoded), tags);
    }

    #[test]
    fn empty_tag_list_encodes_empty() {
        assert_eq!(encode_tags(&[]), "");
        assert!(decode_tags("").is_empty());
    }

    #[test]
    fn whole_tag_match_cannot_match_a_prefix() {
        // `|thread=main|` must not be found inside `|thread=main2|`.
        let encoded = encode_tags(&["thread=main2".to_string()]);
        assert!(!encoded.contains("|thread=main|"));
    }

    #[test]
    fn rejects_separator_quote_and_wildcards() {
        for bad in ["a|b", "a'b", "a%b", "a_b", "a b"] {
            assert!(validate_tag(bad).is_err(), "expected {:?} to be rejected", bad);
        }
    }

    #[test]
    fn accepts_ids_and_key_values() {
        for good in ["kind=message", "agent=12@global", "run=a1b2c3", "thread=main"] {
            assert!(validate_tag(good).is_ok(), "expected {:?} to be accepted", good);
        }
    }

    #[test]
    fn deduplicates_and_bounds_tags() {
        let tags = vec!["a".to_string(), "a".to_string(), "b".to_string()];
        assert_eq!(validate_tags(&tags).unwrap(), vec!["a".to_string(), "b".to_string()]);
        let too_many: Vec<String> = (0..MAX_TAGS + 1).map(|i| format!("t{}", i)).collect();
        assert!(validate_tags(&too_many).is_err());
    }
}
