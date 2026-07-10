//! The VMM HTTP ingress server.
//!
//! A long-lived HTTP listener that accepts requests shaped as
//!
//! ```text
//! {caspar node instance url}/{creatureId}/{programId}/{entityId}/{vmId}/{path…}
//! ```
//!
//! and forwards them to the HTTP server of the VM instance named by `vmId`.
//! The ingress strips the four identity segments, resolves the entity's
//! runtime (and the target instance's container name, when applicable),
//! packages the remaining request into a `forwardHttp` packet, and hands it to
//! the unified VM packet router. The router dispatches it to the entity's plugin
//! through the `caspar_vm_sdk` registry — so no VM type is named here:
//!
//! * five of the six runtimes fall back to the SDK's generic
//!   [`forward_http_via_signal`](caspar_vm_sdk::forward_http_via_signal), which
//!   signals the VM so it handles the request on its next run, and
//! * the docker runtime overrides it to proxy the request straight to the HTTP
//!   server running inside the container and return its real response.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::drivers::vmm::prelude::*;
use crate::models::core::ICore;
use crate::models::transaction::ITrx;
use crate::shell::api::model::Program;

/// Maximum request body the ingress will buffer (16 MiB).
const MAX_BODY: usize = 16 * 1024 * 1024;

pub(crate) struct VmHttpIngress {
    app: Arc<dyn ICore>,
    listening: AtomicBool,
}

impl VmHttpIngress {
    pub(crate) fn new(app: Arc<dyn ICore>) -> Arc<VmHttpIngress> {
        Arc::new(VmHttpIngress {
            app,
            listening: AtomicBool::new(false),
        })
    }

    /// Start the HTTP listener on `0.0.0.0:port` (no-op when `port <= 0` or the
    /// listener is already running). Each accepted connection is served on its
    /// own thread.
    pub(crate) fn listen(self: &Arc<Self>, port: i64) {
        if port <= 0 {
            return;
        }
        if self.listening.swap(true, Ordering::AcqRel) {
            return;
        }
        let ingress = Arc::clone(self);
        thread::spawn(move || {
            let addr = format!("0.0.0.0:{}", port);
            let listener = match TcpListener::bind(&addr) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[vm-http-ingress] bind {} failed: {}", addr, e);
                    ingress.listening.store(false, Ordering::Release);
                    return;
                }
            };
            eprintln!("[vm-http-ingress] listening on {}", addr);
            for incoming in listener.incoming() {
                match incoming {
                    Ok(stream) => {
                        let g = Arc::clone(&ingress);
                        thread::spawn(move || g.handle_connection(stream));
                    }
                    Err(e) => {
                        eprintln!("[vm-http-ingress] accept error: {}", e);
                        continue;
                    }
                }
            }
        });
    }

    fn handle_connection(self: Arc<Self>, mut stream: TcpStream) {
        let req = match parse_request(&mut stream) {
            Ok(r) => r,
            Err(e) => {
                write_response(&mut stream, 400, "Bad Request", "application/json", None,
                    json!({"ok": false, "error": e}).to_string().into_bytes());
                return;
            }
        };

        let (status, reason, content_type, headers, body) = self.route(&req);
        write_response(&mut stream, status, &reason, &content_type, headers, body);
    }

    /// Resolve, package, and forward a parsed request; returns the HTTP
    /// response tuple `(status, reason, content_type, extra_headers, body)`.
    fn route(
        &self,
        req: &HttpRequest,
    ) -> (u16, String, String, Option<Vec<(String, String)>>, Vec<u8>) {
        let seg = match split_identity(&req.path) {
            Some(s) => s,
            None => {
                return (
                    404,
                    "Not Found".to_string(),
                    "application/json".to_string(),
                    None,
                    json!({
                        "ok": false,
                        "error": "path must be /{creatureId}/{programId}/{entityId}/{vmId}/{path…}"
                    })
                    .to_string()
                    .into_bytes(),
                );
            }
        };

        let resolved = self.resolve_target(&seg.program_id, &seg.entity_id, &seg.vm_id);

        let mut headers_map = serde_json::Map::new();
        for (k, v) in &req.headers {
            headers_map.insert(k.clone(), json!(v));
        }

        let packet = json!({
            "type": "forwardHttp",
            "runtime": resolved.runtime,
            "vmType": resolved.runtime,
            "creatureId": seg.creature_id,
            "programId": seg.program_id,
            "machineId": seg.program_id,
            "entityId": seg.entity_id,
            "vmId": seg.vm_id,
            "containerName": resolved.container_name,
            "entityPath": resolved.entity_path,
            "method": req.method,
            "path": seg.rest_path,
            "query": req.query,
            "headers": JsonValue::Object(headers_map),
            "bodyBase64": BASE64_STANDARD.encode(&req.body),
        });

        let raw = crate::drivers::vmm::dispatch_packet(&packet);
        let value: JsonValue = serde_json::from_str(&raw).unwrap_or(JsonValue::Null);

        if value["ok"].as_bool() != Some(true) {
            let err = value["error"]
                .as_str()
                .unwrap_or("forwarding failed")
                .to_string();
            let status = value["status"].as_u64().unwrap_or(502) as u16;
            return (
                status,
                status_reason(status),
                "application/json".to_string(),
                None,
                json!({"ok": false, "error": err}).to_string().into_bytes(),
            );
        }

        let status = value["status"].as_u64().unwrap_or(200) as u16;
        let (content_type, extra_headers) = extract_headers(&value);
        let body = decode_body(&value);
        (status, status_reason(status), content_type, extra_headers, body)
    }

    /// Resolve the runtime, the target VM instance's container name, and the
    /// entity module path from the node state (all best-effort — missing links
    /// simply yield empty fields and the plugin decides how to proceed). The
    /// caller supplies the `vm_id` explicitly (it is a required path segment),
    /// so this never has to guess which instance to target.
    fn resolve_target(&self, program_id: &str, entity_id: &str, vm_id: &str) -> ResolvedTarget {
        let program_id = program_id.to_string();
        let entity_id = entity_id.to_string();
        let vm_id = vm_id.to_string();
        let slot = Arc::new(Mutex::new(ResolvedTarget::default()));
        let slot_c = slot.clone();
        self.app.modify_state(
            true,
            Box::new(move |trx: &dyn ITrx| {
                let mut out = ResolvedTarget::default();

                // Runtime: entity-type link → program record → default.
                let entity_type =
                    trx.get_link(&format!("vmEntityType::{}::{}", program_id, entity_id));
                if !entity_type.trim().is_empty() {
                    out.runtime = entity_type.trim().to_lowercase();
                } else {
                    let program = Program {
                        id: program_id.clone(),
                        ..Default::default()
                    }
                    .pull(trx);
                    if !program.runtime.trim().is_empty() {
                        out.runtime = program.runtime.trim().to_lowercase();
                    } else {
                        out.runtime =
                            caspar_vm_sdk::registry::default_key().unwrap_or_default();
                    }
                }

                // Entity module path (used for artifact-extension resolution).
                let entity_path =
                    trx.get_link(&format!("vmEntityPath::{}::{}", program_id, entity_id));
                if !entity_path.trim().is_empty() {
                    out.entity_path = entity_path;
                }

                // Container name recorded at launch for this specific VM
                // instance, if any (`VmContainerName::{program}::{entity}::{vmId}`).
                if !vm_id.is_empty() {
                    let name = trx.get_link(&format!(
                        "VmContainerName::{}::{}::{}",
                        program_id, entity_id, vm_id
                    ));
                    if !name.trim().is_empty() {
                        out.container_name = name;
                    }
                }

                *slot_c.lock().unwrap() = out;
                Ok(())
            }),
        );
        let resolved = slot.lock().unwrap().clone();
        resolved
    }
}

#[derive(Clone, Default)]
struct ResolvedTarget {
    runtime: String,
    container_name: String,
    entity_path: String,
}

struct HttpRequest {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct IdentitySegments {
    creature_id: String,
    program_id: String,
    entity_id: String,
    vm_id: String,
    rest_path: String,
}

/// Split a request path into
/// `/{creatureId}/{programId}/{entityId}/{vmId}/{rest…}`. All four identity
/// segments are required; the VM id names the specific instance the request is
/// forwarded to.
fn split_identity(path: &str) -> Option<IdentitySegments> {
    let trimmed = path.trim_start_matches('/');
    let mut it = trimmed.splitn(5, '/');
    let creature_id = it.next().filter(|s| !s.is_empty())?.to_string();
    let program_id = it.next().filter(|s| !s.is_empty())?.to_string();
    let entity_id = it.next().filter(|s| !s.is_empty())?.to_string();
    let vm_id = it.next().filter(|s| !s.is_empty())?.to_string();
    let rest = it.next().unwrap_or("");
    let rest_path = format!("/{}", rest);
    Some(IdentitySegments {
        creature_id,
        program_id,
        entity_id,
        vm_id,
        rest_path,
    })
}

/// Parse a minimal HTTP/1.1 request off `stream`: request line, headers, and a
/// Content-Length-bounded body.
fn parse_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);

    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|e| format!("read request line: {}", e))?;
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Err("malformed request line".to_string());
    }
    let method = parts[0].to_string();
    let target = parts[1].to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("read header: {}", e))?;
        if n == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_string();
            let val = v.trim().to_string();
            if key.eq_ignore_ascii_case("content-length") {
                content_length = val.parse::<usize>().unwrap_or(0).min(MAX_BODY);
            }
            headers.push((key, val));
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|e| format!("read body: {}", e))?;
    }

    Ok(HttpRequest {
        method,
        path,
        query,
        headers,
        body,
    })
}

/// Read `content-type` and any other headers a plugin returned in its response.
fn extract_headers(value: &JsonValue) -> (String, Option<Vec<(String, String)>>) {
    let mut content_type = "application/octet-stream".to_string();
    let mut extra: Vec<(String, String)> = Vec::new();
    if let Some(obj) = value["headers"].as_object() {
        for (k, v) in obj {
            let val = match v.as_str() {
                Some(s) => s.to_string(),
                None => v.to_string(),
            };
            if k.eq_ignore_ascii_case("content-type") {
                content_type = val;
            } else if k.eq_ignore_ascii_case("content-length")
                || k.eq_ignore_ascii_case("transfer-encoding")
                || k.eq_ignore_ascii_case("connection")
            {
                // Recomputed by the ingress; never pass through.
                continue;
            } else {
                extra.push((k.clone(), val));
            }
        }
    }
    let extra = if extra.is_empty() { None } else { Some(extra) };
    (content_type, extra)
}

/// Decode a plugin response body from `bodyBase64` (preferred) or `body`.
fn decode_body(value: &JsonValue) -> Vec<u8> {
    if let Some(b64) = value["bodyBase64"].as_str() {
        if let Ok(bytes) = BASE64_STANDARD.decode(b64) {
            return bytes;
        }
    }
    match value["body"].as_str() {
        Some(s) => s.as_bytes().to_vec(),
        None if value["body"].is_null() => Vec::new(),
        None => value["body"].to_string().into_bytes(),
    }
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    extra_headers: Option<Vec<(String, String)>>,
    body: Vec<u8>,
) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        status,
        reason,
        content_type,
        body.len()
    );
    if let Some(hs) = extra_headers {
        for (k, v) in hs {
            // Drop header values that would break the framing.
            if v.contains('\r') || v.contains('\n') {
                continue;
            }
            head.push_str(&format!("{}: {}\r\n", k, v));
        }
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

fn status_reason(status: u16) -> String {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "OK",
    }
    .to_string()
}
