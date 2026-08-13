//! Custom VM gateway routing by creature username + a deployer-defined path.
//!
//! Besides the fully-qualified ingress form
//! `/{creatureId}/{programId}/{entityId}/{vmId}/{path…}`, a deployer may attach
//! a **custom path** to a VM entity at deploy time so the entity's HTTP server
//! is reachable through the friendly form
//!
//! ```text
//! {caspar node instance url}/{creatureUsername}/{customPath…}
//! ```
//!
//! The custom path is chosen by the entity's deployer (metadata `gatewayPath`)
//! and stored on-chain as a link keyed by the owning creature id and the
//! normalized path prefix. At request time the ingress resolves the leading
//! path segment as a creature username, then matches the longest registered
//! prefix for that creature to recover the target `{programId, entityId, vmId}`
//! and the remaining sub-path to forward to the VM. This keeps the routing
//! table on chain (so it replicates with the deploy) and lets one creature
//! expose several entities under different paths.

use serde_json::{json, Value as JsonValue};

/// Link key prefix for a custom route: `vmHttpRoute::<creatureId>::<path>`.
pub const ROUTE_LINK_NS: &str = "vmHttpRoute";
/// Reverse pointer prefix: `vmHttpRouteFor::<programId>::<entityId>` → the
/// `<creatureId>::<path>` currently registered for that entity, so a redeploy
/// that changes (or clears) the path can drop the stale route first.
pub const ROUTE_REV_LINK_NS: &str = "vmHttpRouteFor";

/// Alias prefix: `vmHttpRouteUser::<usernameLocalPart>` → `<creatureId>`. Lets a
/// request address the creature by the bare local part of its username (the part
/// before `@<source>`), e.g. `/m-tool-github/…` instead of the full
/// `/name@http://host:port/…` (which cannot go in a URL path) or the numeric id.
/// Written when a route is registered, so resolution stays a single link read.
pub const ROUTE_ALIAS_LINK_NS: &str = "vmHttpRouteUser";

/// Maximum number of path segments a custom route prefix may span. Requests are
/// matched longest-prefix-first over at most this many leading segments, so the
/// per-request work is bounded regardless of the request path length.
pub const MAX_ROUTE_SEGMENTS: usize = 8;

/// Normalize a custom path into its canonical prefix form: no leading/trailing
/// slashes and no empty segments. `"/api/v1/"` and `"api//v1"` both become
/// `"api/v1"`. Returns an empty string when nothing is left, which callers
/// treat as "no route".
pub fn normalize_path(path: &str) -> String {
    path.split('/')
        .filter(|seg| !seg.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// The on-chain link key holding the route for `creature_id` at `path`.
pub fn route_link_key(creature_id: &str, path: &str) -> String {
    format!("{}::{}::{}", ROUTE_LINK_NS, creature_id, path)
}

/// The reverse pointer link key for an entity's currently registered route.
pub fn route_rev_link_key(program_id: &str, entity_id: &str) -> String {
    format!("{}::{}::{}", ROUTE_REV_LINK_NS, program_id, entity_id)
}

/// The alias link key mapping a username's bare local part to a creature id.
pub fn route_alias_link_key(local_part: &str) -> String {
    format!("{}::{}", ROUTE_ALIAS_LINK_NS, local_part)
}

/// The local part of a creature username (`name@source` → `name`). Returns the
/// whole string when there is no `@`.
pub fn username_local_part(username: &str) -> &str {
    username.split('@').next().unwrap_or(username)
}

/// Encode a route target as the value stored under [`route_link_key`].
pub fn encode_target(program_id: &str, entity_id: &str, vm_id: &str) -> String {
    json!({
        "programId": program_id,
        "entityId": entity_id,
        "vmId": vm_id,
    })
    .to_string()
}

/// A resolved custom route: the identity the request forwards to plus the
/// sub-path (after the matched prefix) to hand the VM.
pub struct ResolvedRoute {
    pub program_id: String,
    pub entity_id: String,
    pub vm_id: String,
    /// Forwarded request path, always leading-slash prefixed (`"/"` when the
    /// request targeted the route root).
    pub rest_path: String,
}

/// Parse a stored route value plus the leftover request segments into a
/// [`ResolvedRoute`]. `rest_segments` is what followed the matched prefix.
pub fn decode_target(stored: &str, rest_segments: &[&str]) -> Option<ResolvedRoute> {
    let value: JsonValue = serde_json::from_str(stored).ok()?;
    let program_id = value["programId"].as_str().unwrap_or("").to_string();
    let entity_id = value["entityId"].as_str().unwrap_or("").to_string();
    if program_id.is_empty() || entity_id.is_empty() {
        return None;
    }
    let vm_id = value["vmId"].as_str().unwrap_or("").to_string();
    let rest_path = format!("/{}", rest_segments.join("/"));
    Some(ResolvedRoute {
        program_id,
        entity_id,
        vm_id,
        rest_path,
    })
}
