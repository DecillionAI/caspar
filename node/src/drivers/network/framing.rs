//! Shared TLS + length-prefixed framing helpers used by the TCP / WS / WS-fed
//! drivers. Provides a thin sync wrapper around `rustls` plus encode/decode
//! functions for the Go socket framing the Caspar clients speak:
//!
//! ```text
//!   request   : u32be(len) || u8(0x03) || u32be|signature || u32be|userId
//!                                       || u32be|path      || u32be|packetId
//!                                       || payload
//!   response  : u32be(len) || u8(0x02) || u32be|requestId  || u32be(resCode)
//!                                       || (federation: u32be|signature)
//!                                       || payload
//!   update    : u32be(len) || u8(0x01) || u32be|key        || payload
//!                                       (federation adds signature/target/exc)
//! ```
//!
//! The framing helpers below operate at the inner layer — `decode_inner_frame`
//! is handed the bytes *after* the outer u32 length is stripped, and
//! `encode_*` produce the same. The drivers wrap them in a length prefix when
//! sending and parse a length prefix before calling `decode_*`.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, ClientConnection, ServerConfig, ServerConnection, StreamOwned};

use crate::models::ports::network::TlsConfig;

/// Maximum frame size we accept — matches the Go driver's `20_000_000`
/// safety cap.
pub const MAX_FRAME_LEN: u32 = 20_000_000;

/// A TCP connection, optionally wrapped in a `rustls::StreamOwned`.
pub enum TlsStream {
    Plain(TcpStream),
    Server(Box<StreamOwned<ServerConnection, TcpStream>>),
    Client(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl TlsStream {
    /// Get the peer address, or "<unknown>" if it can't be resolved.
    pub fn peer_addr(&self) -> String {
        let stream = match self {
            TlsStream::Plain(s) => s,
            TlsStream::Server(s) => &s.sock,
            TlsStream::Client(s) => &s.sock,
        };
        stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "<unknown>".to_string())
    }

    /// Best-effort shutdown.
    pub fn shutdown(&mut self) {
        let stream = match self {
            TlsStream::Plain(s) => s,
            TlsStream::Server(s) => &mut s.sock,
            TlsStream::Client(s) => &mut s.sock,
        };
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

impl Read for TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            TlsStream::Plain(s) => s.read(buf),
            TlsStream::Server(s) => s.read(buf),
            TlsStream::Client(s) => s.read(buf),
        }
    }
}

impl Write for TlsStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            TlsStream::Plain(s) => s.write(buf),
            TlsStream::Server(s) => s.write(buf),
            TlsStream::Client(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            TlsStream::Plain(s) => s.flush(),
            TlsStream::Server(s) => s.flush(),
            TlsStream::Client(s) => s.flush(),
        }
    }
}

// -- Listener / dialer ------------------------------------------------------

/// `bind_tls(port, cfg)` opens a TCP listener and, if `cfg` is provided,
/// returns the `rustls::ServerConfig` used to wrap accepted connections.
pub fn bind_tls(port: i64, cfg: Option<&TlsConfig>) -> Result<(TcpListener, Option<Arc<ServerConfig>>)> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port))
        .map_err(|e| anyhow!("tcp listen :{}: {}", port, e))?;
    let server = if let Some(cfg) = cfg {
        Some(Arc::new(build_server_config(cfg)?))
    } else {
        None
    };
    Ok((listener, server))
}

/// Accept the next inbound connection, optionally completing a TLS handshake.
pub fn accept(
    listener: &TcpListener,
    cfg: Option<&Arc<ServerConfig>>,
) -> Result<TlsStream> {
    let (stream, _) = listener.accept().map_err(|e| anyhow!("accept: {}", e))?;
    stream
        .set_nodelay(true)
        .map_err(|e| anyhow!("set_nodelay: {}", e))?;
    match cfg {
        Some(c) => {
            let conn = ServerConnection::new(c.clone())
                .map_err(|e| anyhow!("server connection: {}", e))?;
            Ok(TlsStream::Server(Box::new(StreamOwned::new(conn, stream))))
        }
        None => Ok(TlsStream::Plain(stream)),
    }
}

/// Open an outbound TCP connection. If `tls` is `Some`, wraps it in a
/// `rustls::ClientConnection` using `ServerName` derived from `host`.
pub fn dial(addr: &str, tls: Option<&TlsConfig>) -> Result<TlsStream> {
    let stream = TcpStream::connect(addr).map_err(|e| anyhow!("dial {}: {}", addr, e))?;
    stream.set_nodelay(true).ok();
    match tls {
        Some(cfg) => {
            let client_cfg = build_client_config(cfg)?;
            let host = cfg.server_name.clone();
            let host = if host.is_empty() {
                addr.split(':').next().unwrap_or(addr).to_string()
            } else {
                host
            };
            let server_name = ServerName::try_from(host)
                .map_err(|e| anyhow!("server name: {}", e))?;
            let conn = ClientConnection::new(Arc::new(client_cfg), server_name)
                .map_err(|e| anyhow!("client connection: {}", e))?;
            Ok(TlsStream::Client(Box::new(StreamOwned::new(conn, stream))))
        }
        None => Ok(TlsStream::Plain(stream)),
    }
}

fn build_server_config(cfg: &TlsConfig) -> Result<ServerConfig> {
    ensure_default_crypto_provider();
    let certs = parse_certs(&cfg.cert_pem)?;
    let key = parse_private_key(&cfg.key_pem)?;
    let mut sc = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| anyhow!("server tls cert: {}", e))?;
    sc.alpn_protocols.clear();
    Ok(sc)
}

fn build_client_config(cfg: &TlsConfig) -> Result<ClientConfig> {
    ensure_default_crypto_provider();
    let mut roots = rustls::RootCertStore::empty();
    if !cfg.ca_pem.is_empty() {
        for cert in parse_certs(&cfg.ca_pem)? {
            let _ = roots.add(cert);
        }
    } else {
        // Fall back to webpki-roots equivalent — for our intra-org TLS the
        // peers usually share a CA-pinning channel. We can't load system
        // roots without an extra crate, so callers that need them must
        // populate `ca_pem` explicitly.
    }
    let builder = ClientConfig::builder().with_root_certificates(roots);
    let cc = if cfg.insecure_skip_verify {
        // SAFETY: the dangerous_no_check verifier is used only when the
        // caller explicitly asked for it. The Go driver had the equivalent
        // flag (`signal_skip_verify`).
        let mut cc = builder.with_no_client_auth();
        cc.dangerous()
            .set_certificate_verifier(Arc::new(NoCertVerifier));
        cc
    } else {
        builder.with_no_client_auth()
    };
    Ok(cc)
}

fn parse_certs(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>> {
    let mut reader = std::io::Cursor::new(pem);
    rustls_pemfile::certs(&mut reader)
        .map(|r| r.map_err(|e| anyhow!("pem cert: {}", e)))
        .collect()
}

fn parse_private_key(pem: &[u8]) -> Result<PrivateKeyDer<'static>> {
    let mut reader = std::io::Cursor::new(pem);
    // Try PKCS#8 first, then PKCS#1, then SEC1.
    if let Some(Ok(key)) = rustls_pemfile::pkcs8_private_keys(&mut reader).next() {
        return Ok(PrivateKeyDer::Pkcs8(key));
    }
    let mut reader = std::io::Cursor::new(pem);
    if let Some(Ok(key)) = rustls_pemfile::rsa_private_keys(&mut reader).next() {
        return Ok(PrivateKeyDer::Pkcs1(key));
    }
    let mut reader = std::io::Cursor::new(pem);
    if let Some(Ok(key)) = rustls_pemfile::ec_private_keys(&mut reader).next() {
        return Ok(PrivateKeyDer::Sec1(key));
    }
    Err(anyhow!("no private key found in PEM"))
}

/// `tls_config_from_files(cert, key)` loads a PEM-encoded cert + key pair.
pub fn tls_config_from_files(cert_path: &str, key_path: &str) -> Result<TlsConfig> {
    Ok(TlsConfig {
        cert_pem: fs::read(cert_path).map_err(|e| anyhow!("read cert: {}", e))?,
        key_pem: fs::read(key_path).map_err(|e| anyhow!("read key: {}", e))?,
        ..Default::default()
    })
}

fn ensure_default_crypto_provider() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Permissive certificate verifier used only when the caller asked for
/// `insecure_skip_verify` (mirrors the same flag from Go).
#[derive(Debug)]
struct NoCertVerifier;

impl rustls::client::danger::ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}

// -- Inner-frame helpers ----------------------------------------------------

/// Read `n` bytes from `r` into `out`, returning `None` on EOF (so the read
/// loop knows to terminate cleanly).
pub fn read_exact_opt(r: &mut dyn Read, out: &mut [u8]) -> Result<Option<()>> {
    let mut filled = 0;
    while filled < out.len() {
        match r.read(&mut out[filled..]) {
            Ok(0) => return Ok(None),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(anyhow!("read: {}", e)),
        }
    }
    Ok(Some(()))
}

/// Read the outer length-prefixed frame and return its body.
pub fn read_length_prefixed_frame(r: &mut dyn Read) -> Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match read_exact_opt(r, &mut len_buf)? {
        Some(()) => {}
        None => return Ok(None),
    }
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(anyhow!("frame too large: {}", len));
    }
    let mut body = vec![0u8; len as usize];
    if read_exact_opt(r, &mut body)?.is_none() {
        return Ok(None);
    }
    Ok(Some(body))
}

/// Write `body` length-prefixed.
pub fn write_length_prefixed_frame(w: &mut dyn Write, body: &[u8]) -> Result<()> {
    let len = body.len() as u32;
    if len > MAX_FRAME_LEN {
        return Err(anyhow!("frame too large: {}", len));
    }
    w.write_all(&len.to_be_bytes())
        .map_err(|e| anyhow!("write len: {}", e))?;
    w.write_all(body).map_err(|e| anyhow!("write body: {}", e))?;
    w.flush().map_err(|e| anyhow!("flush: {}", e))?;
    Ok(())
}

/// Parsed request frame: `[0x03][sig][userId][path][packetId][payload]`.
pub struct RequestFrame {
    pub signature: String,
    pub user_id: String,
    pub path: String,
    pub packet_id: String,
    pub payload: Vec<u8>,
}

/// Parsed response frame: `[0x02][packetId][resCode][?signature][payload]`.
pub struct ResponseFrame {
    pub packet_id: String,
    pub res_code: i64,
    pub signature: String,
    pub payload: Vec<u8>,
}

/// Parsed update frame: `[0x01][?signature][?targetId][?exceptions][key][payload]`.
pub struct UpdateFrame {
    pub signature: String,
    pub target_id: String,
    pub exceptions: Vec<String>,
    pub key: String,
    pub payload: Vec<u8>,
}

/// Decode a length-prefixed field: u32be length + n bytes.
fn read_lp_field(frame: &[u8], pointer: &mut usize) -> Result<Vec<u8>> {
    if *pointer + 4 > frame.len() {
        return Err(anyhow!("truncated frame at lp len"));
    }
    let len = u32::from_be_bytes(frame[*pointer..*pointer + 4].try_into().unwrap());
    *pointer += 4;
    if len > MAX_FRAME_LEN {
        return Err(anyhow!("lp field too large: {}", len));
    }
    if *pointer + len as usize > frame.len() {
        return Err(anyhow!("truncated frame at lp body"));
    }
    let v = frame[*pointer..*pointer + len as usize].to_vec();
    *pointer += len as usize;
    Ok(v)
}

/// Decode the post-tag bytes of a request frame.
pub fn decode_request_body(body: &[u8]) -> Result<RequestFrame> {
    let mut p = 0;
    let signature = String::from_utf8_lossy(&read_lp_field(body, &mut p)?).into_owned();
    let user_id = String::from_utf8_lossy(&read_lp_field(body, &mut p)?).into_owned();
    let path = String::from_utf8_lossy(&read_lp_field(body, &mut p)?).into_owned();
    let packet_id = String::from_utf8_lossy(&read_lp_field(body, &mut p)?).into_owned();
    let payload = body[p..].to_vec();
    Ok(RequestFrame {
        signature,
        user_id,
        path,
        packet_id,
        payload,
    })
}

/// Decode the post-tag bytes of a federation-flavoured response frame.
pub fn decode_response_body(body: &[u8], has_signature: bool) -> Result<ResponseFrame> {
    let mut p = 0;
    let packet_id = String::from_utf8_lossy(&read_lp_field(body, &mut p)?).into_owned();
    if p + 4 > body.len() {
        return Err(anyhow!("truncated frame at resCode"));
    }
    let res_code = u32::from_be_bytes(body[p..p + 4].try_into().unwrap()) as i64;
    p += 4;
    let signature = if has_signature {
        String::from_utf8_lossy(&read_lp_field(body, &mut p)?).into_owned()
    } else {
        String::new()
    };
    let payload = body[p..].to_vec();
    Ok(ResponseFrame {
        packet_id,
        res_code,
        signature,
        payload,
    })
}

/// Decode the post-tag bytes of an update frame.
///
/// For the basic client driver `has_metadata == false`: only key + payload.
/// For the federation driver `has_metadata == true`: signature + targetId +
/// exceptions JSON + key + payload.
pub fn decode_update_body(body: &[u8], has_metadata: bool) -> Result<UpdateFrame> {
    let mut p = 0;
    let mut signature = String::new();
    let mut target_id = String::new();
    let mut exceptions: Vec<String> = Vec::new();
    if has_metadata {
        signature = String::from_utf8_lossy(&read_lp_field(body, &mut p)?).into_owned();
        target_id = String::from_utf8_lossy(&read_lp_field(body, &mut p)?).into_owned();
        let exc_bytes = read_lp_field(body, &mut p)?;
        exceptions = serde_json::from_slice::<Vec<String>>(&exc_bytes).unwrap_or_default();
    }
    let key = String::from_utf8_lossy(&read_lp_field(body, &mut p)?).into_owned();
    let payload = body[p..].to_vec();
    Ok(UpdateFrame {
        signature,
        target_id,
        exceptions,
        key,
        payload,
    })
}

/// Encode a request frame (federation outbound). Layout:
/// `[0x03][sig][userId][path][packetId][payload]`.
pub fn encode_request_body(
    signature: &str,
    user_id: &str,
    path: &str,
    packet_id: &str,
    payload: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        1 + 4 + signature.len() + 4 + user_id.len() + 4 + path.len() + 4 + packet_id.len() + payload.len(),
    );
    out.push(0x03);
    write_lp_str(&mut out, signature);
    write_lp_str(&mut out, user_id);
    write_lp_str(&mut out, path);
    write_lp_str(&mut out, packet_id);
    out.extend_from_slice(payload);
    out
}

/// Encode a response frame (client). Layout: `[0x02][packetId][resCode][payload]`.
pub fn encode_client_response_body(packet_id: &str, res_code: i64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 4 + packet_id.len() + 4 + payload.len());
    out.push(0x02);
    write_lp_str(&mut out, packet_id);
    out.extend_from_slice(&(res_code as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Encode a federation response frame: `[0x02][packetId][resCode][sig][payload]`.
pub fn encode_fed_response_body(
    packet_id: &str,
    res_code: i64,
    signature: &str,
    payload: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        1 + 4 + packet_id.len() + 4 + 4 + signature.len() + payload.len(),
    );
    out.push(0x02);
    write_lp_str(&mut out, packet_id);
    out.extend_from_slice(&(res_code as u32).to_be_bytes());
    write_lp_str(&mut out, signature);
    out.extend_from_slice(payload);
    out
}

/// Encode an update frame (client). Layout: `[0x01][key][payload]`.
pub fn encode_client_update_body(key: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 4 + key.len() + payload.len());
    out.push(0x01);
    write_lp_str(&mut out, key);
    out.extend_from_slice(payload);
    out
}

/// Encode a federation update frame:
/// `[0x01][sig][targetId][exceptions][key][payload]`.
pub fn encode_fed_update_body(
    signature: &str,
    target_id: &str,
    exceptions: &[String],
    key: &str,
    payload: &[u8],
) -> Vec<u8> {
    let exc_json = serde_json::to_vec(exceptions).unwrap_or_else(|_| b"[]".to_vec());
    let mut out = Vec::with_capacity(
        1 + 4 + signature.len() + 4 + target_id.len() + 4 + exc_json.len()
            + 4 + key.len() + payload.len(),
    );
    out.push(0x01);
    write_lp_str(&mut out, signature);
    write_lp_str(&mut out, target_id);
    out.extend_from_slice(&(exc_json.len() as u32).to_be_bytes());
    out.extend_from_slice(&exc_json);
    write_lp_str(&mut out, key);
    out.extend_from_slice(payload);
    out
}

fn write_lp_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}
