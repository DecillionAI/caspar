//! Shared low-level helpers used across the translated Caspar node.

/// TCP keep-alive configuration applied to every long-lived stream owned by
/// the Caspar node: client TCP, client WS, federation, and the babble chain
/// transport. Without these settings the Linux kernel waits two hours before
/// sending the first probe, so a peer (NAT box, container, idle deploy
/// client) that goes silent leaves us with a half-open socket that only
/// reveals itself the next time we try to write — by which point the
/// outbound queue is already full and the deploy script has hit its own
/// read timeout. With these settings the kernel notices a dead peer in
/// roughly a minute and closes the socket so our accept / read loops can
/// reclaim resources promptly.
pub mod keepalive {
    use std::net::TcpStream;
    use std::os::unix::io::AsRawFd;

    /// Idle period after which the first probe is sent, in seconds.
    /// Short enough that a transient deploy stall (which is the failure mode
    /// the benchmark workflow keeps tripping over) is visible quickly,
    /// long enough that healthy idle connections (e.g. the WS-update bus)
    /// are not woken up unnecessarily.
    const IDLE_SECS: i32 = 30;
    /// Spacing between probes, in seconds.
    const INTVL_SECS: i32 = 10;
    /// Number of unanswered probes before the kernel declares the socket
    /// dead and surfaces an error from the next read/write.
    const CNT: i32 = 3;

    /// Enable SO_KEEPALIVE on the stream and install the per-socket idle /
    /// interval / count overrides. Best-effort: a missing TCP_KEEPIDLE on
    /// an exotic platform leaves SO_KEEPALIVE on with the kernel default,
    /// which is still better than no keepalive at all.
    pub fn apply(stream: &TcpStream) {
        let fd = stream.as_raw_fd();
        unsafe {
            let one: libc::c_int = 1;
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_KEEPALIVE,
                &one as *const _ as *const libc::c_void,
                std::mem::size_of_val(&one) as libc::socklen_t,
            );
            let idle: libc::c_int = IDLE_SECS;
            libc::setsockopt(
                fd,
                libc::IPPROTO_TCP,
                libc::TCP_KEEPIDLE,
                &idle as *const _ as *const libc::c_void,
                std::mem::size_of_val(&idle) as libc::socklen_t,
            );
            let intvl: libc::c_int = INTVL_SECS;
            libc::setsockopt(
                fd,
                libc::IPPROTO_TCP,
                libc::TCP_KEEPINTVL,
                &intvl as *const _ as *const libc::c_void,
                std::mem::size_of_val(&intvl) as libc::socklen_t,
            );
            let cnt: libc::c_int = CNT;
            libc::setsockopt(
                fd,
                libc::IPPROTO_TCP,
                libc::TCP_KEEPCNT,
                &cnt as *const _ as *const libc::c_void,
                std::mem::size_of_val(&cnt) as libc::socklen_t,
            );
        }
    }

    /// Interval at which application-level WebSocket Ping frames are sent
    /// when a connection has been idle. WebSocket has its own ping/pong
    /// framing that lives above TCP keep-alive — proxies and middleboxes
    /// that strip TCP keep-alive (or buffer it indefinitely) still respect
    /// WS frames, so a higher-level ping is a useful belt-and-braces
    /// complement on the WS driver.
    pub const WS_PING_INTERVAL_SECS: u64 = 25;
}

/// Bounded exponential-back-off retry primitive. Used by every outbound
/// dial / connect site in the node so the policy is consistent: short
/// transient outages (peer container restart, listener race with
/// accept) are absorbed without disturbing higher-level callers, but a
/// genuinely-dead peer still surfaces within a couple of seconds.
pub mod retry {
    use std::thread;
    use std::time::Duration;

    /// Run `f` up to `attempts` times, sleeping `base * 2^attempt`
    /// between attempts. Returns the first `Ok` value, or the final
    /// `Err` if every attempt fails. `attempts == 0` returns the final
    /// `Err` from a synthetic exhausted-attempts run; callers should
    /// pass at least 1.
    pub fn retry_backoff<T, E, F>(attempts: u32, base: Duration, mut f: F) -> Result<T, E>
    where
        F: FnMut() -> Result<T, E>,
    {
        let mut last_err: Option<E> = None;
        for attempt in 0..attempts.max(1) {
            match f() {
                Ok(v) => return Ok(v),
                Err(e) => {
                    last_err = Some(e);
                    if attempt + 1 >= attempts {
                        break;
                    }
                    thread::sleep(base.saturating_mul(1u32 << attempt));
                }
            }
        }
        Err(last_err.expect("attempts >= 1 guarantees at least one Err"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicU32, Ordering};

        #[test]
        fn succeeds_on_first_attempt() {
            let calls = AtomicU32::new(0);
            let result: Result<i32, &str> = retry_backoff(3, Duration::from_millis(1), || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            });
            assert_eq!(result, Ok(42));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn succeeds_after_two_failures() {
            let calls = AtomicU32::new(0);
            let result: Result<i32, &str> = retry_backoff(3, Duration::from_millis(1), || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err("not yet")
                } else {
                    Ok(99)
                }
            });
            assert_eq!(result, Ok(99));
            assert_eq!(calls.load(Ordering::SeqCst), 3);
        }

        #[test]
        fn returns_final_err_after_exhausting_attempts() {
            let calls = AtomicU32::new(0);
            let result: Result<i32, &str> = retry_backoff(3, Duration::from_millis(1), || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err("always fails")
            });
            assert_eq!(result, Err("always fails"));
            assert_eq!(calls.load(Ordering::SeqCst), 3);
        }
    }
}


/// Serde helpers that (de)serialize `Vec<u8>` as a standard base64 string,
/// matching Go's `encoding/json` behaviour for `[]byte` fields.
pub mod bytes_base64 {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let opt = Option::<String>::deserialize(d)?;
        match opt {
            None => Ok(Vec::new()),
            Some(s) => STANDARD.decode(s.as_bytes()).map_err(serde::de::Error::custom),
        }
    }
}

/// Serde helpers that (de)serialize `Vec<Vec<u8>>` as a JSON array of base64
/// strings, matching Go's `encoding/json` behaviour for `[][]byte` fields.
pub mod bytes_base64_vec {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::ser::SerializeSeq;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[Vec<u8>], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(v.len()))?;
        for item in v {
            seq.serialize_element(&STANDARD.encode(item))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<u8>>, D::Error> {
        let opt = Option::<Vec<String>>::deserialize(d)?;
        match opt {
            None => Ok(Vec::new()),
            Some(strs) => strs
                .iter()
                .map(|s| STANDARD.decode(s).map_err(serde::de::Error::custom))
                .collect(),
        }
    }
}

/// Clones a `OnceLock`, preserving an already-initialised value. Used to make
/// translated structs that carry lazy caches cloneable.
pub fn clone_once_lock<T: Clone>(o: &std::sync::OnceLock<T>) -> std::sync::OnceLock<T> {
    let new = std::sync::OnceLock::new();
    if let Some(v) = o.get() {
        let _ = new.set(v.clone());
    }
    new
}

/// Convenience alias mirroring Go's `error` value type.
pub type GoError = anyhow::Error;

/// Opaque value, the translation of Go's empty interface `interface{}` / `any`
/// when it is used for dynamic, downcastable values rather than JSON payloads.
pub type AnyVal = std::sync::Arc<dyn std::any::Any + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct OneBlob {
        #[serde(with = "bytes_base64", default)]
        bytes: Vec<u8>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct ManyBlobs {
        #[serde(with = "bytes_base64_vec", default)]
        blobs: Vec<Vec<u8>>,
    }

    #[test]
    fn bytes_base64_serializes_to_standard_base64() {
        let v = OneBlob { bytes: vec![0u8, 1, 2, 3, 255] };
        let s = serde_json::to_string(&v).unwrap();
        // AAECA/8= is the std-base64 encoding of [0,1,2,3,255].
        assert_eq!(s, r#"{"bytes":"AAECA/8="}"#);
    }

    #[test]
    fn bytes_base64_round_trips_empty_and_nonempty() {
        for payload in [vec![], vec![1, 2, 3], (0u8..=255).collect::<Vec<u8>>()] {
            let v = OneBlob { bytes: payload.clone() };
            let s = serde_json::to_string(&v).unwrap();
            let parsed: OneBlob = serde_json::from_str(&s).unwrap();
            assert_eq!(parsed.bytes, payload);
        }
    }

    #[test]
    fn bytes_base64_accepts_json_null_as_empty() {
        let parsed: OneBlob = serde_json::from_str(r#"{"bytes":null}"#).unwrap();
        assert!(parsed.bytes.is_empty());
    }

    #[test]
    fn bytes_base64_rejects_invalid_base64() {
        let err = serde_json::from_str::<OneBlob>(r#"{"bytes":"!!not base64!!"}"#)
            .expect_err("invalid base64 should error");
        let msg = format!("{}", err);
        assert!(msg.to_lowercase().contains("invalid") || msg.to_lowercase().contains("symbol"));
    }

    #[test]
    fn bytes_base64_vec_round_trips() {
        let v = ManyBlobs {
            blobs: vec![vec![], vec![0xde, 0xad], vec![0xbe, 0xef]],
        };
        let s = serde_json::to_string(&v).unwrap();
        let parsed: ManyBlobs = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.blobs, v.blobs);
    }

    #[test]
    fn bytes_base64_vec_accepts_null() {
        let parsed: ManyBlobs = serde_json::from_str(r#"{"blobs":null}"#).unwrap();
        assert!(parsed.blobs.is_empty());
    }

    #[test]
    fn clone_once_lock_preserves_initialised_value() {
        let lock: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        lock.set("hello".to_string()).unwrap();
        let cloned = clone_once_lock(&lock);
        assert_eq!(cloned.get(), Some(&"hello".to_string()));
    }

    #[test]
    fn clone_once_lock_starts_uninitialised_when_empty() {
        let lock: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        let cloned = clone_once_lock(&lock);
        assert!(cloned.get().is_none());
        cloned.set(7).unwrap();
        assert_eq!(cloned.get(), Some(&7));
    }
}
