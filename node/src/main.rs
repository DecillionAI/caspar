// Caspar node — Rust translation of the Caspar (kasper) Go node.

#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(clippy::module_inception)]
#![allow(clippy::type_complexity)]

#[macro_use]
mod compat;

mod bots;
mod core;
mod drivers;
mod models;
mod shell;
mod telemetry;
mod util;

use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::models::action::ExtendedField;
use crate::models::core::ICore;
use crate::models::transaction::ITrx;
use crate::shell::api::main_api::plug_all;
use crate::shell::kasper::new_app;

fn main() {
    // .env loading: simple manual parser (skip dotenvy to avoid the extra
    // dependency — the file lives next to the binary in production).
    let _ = load_dotenv(".env");

    // Runtime profiling HTTP server (was Go `net/http/pprof` on :9999;
    // now Rust-native via the `pprof` crate). Queried by `casparctl pprof`.
    telemetry::pprof::start_from_env();

    if let Err(e) = telemetry::start_from_env() {
        eprintln!("telemetry server start failed: {}", e);
    }

    let owner_id = env::var("OWNER_ID").unwrap_or_default();
    let owner_pem = env::var("OWNER_PRIVATE_KEY").unwrap_or_default();
    let owner_priv = match parse_owner_key(&owner_pem) {
        Some(k) => k,
        None => {
            eprintln!("OWNER_PRIVATE_KEY missing or unparseable");
            return;
        }
    };
    let origin = env::var("ORIGIN").unwrap_or_default();
    let app = new_app(&origin, &owner_id, owner_priv);

    let storage_root = env::var("STORAGE_ROOT_PATH").unwrap_or_default();
    let base_db_path = env::var("BASE_DB_PATH").unwrap_or_default();
    let applet_db_path = env::var("APPLET_DB_PATH").unwrap_or_default();
    let store_logs_db = env::var("STORE_LOGS_DB").unwrap_or_default();
    let searcher_db = env::var("SEARCH_INDEX_PATH").unwrap_or_default();

    if let Err(e) = app.load_inner(
        vec!["keyhan".to_string()],
        &storage_root,
        &base_db_path,
        &applet_db_path,
        &store_logs_db,
        &searcher_db,
    ) {
        eprintln!("app.load failed: {}", e);
        return;
    }

    // Install SIGINT / SIGTERM handler: when received, close the app and
    // exit. We use a small helper instead of pulling in signal-hook.
    install_signal_handler({
        let app = app.clone();
        move || {
            app.close();
            std::process::exit(0);
        }
    });

    let mut user_extender: HashMap<String, ExtendedField> = HashMap::new();
    let make_field =
        |name: &str, default: serde_json::Value, searchable: bool, primary: bool| ExtendedField {
            name: name.to_string(),
            path: "metadata.public.profile".to_string(),
            typ: "string".to_string(),
            default,
            required: true,
            searchable,
            primary_prop: primary,
            get_value: None,
        };
    user_extender.insert(
        "name".to_string(),
        make_field("name", serde_json::json!("Anonymous User"), true, true),
    );
    user_extender.insert(
        "avatar".to_string(),
        make_field("avatar", serde_json::json!("avatar"), false, true),
    );
    user_extender.insert(
        "bio".to_string(),
        make_field("bio", serde_json::json!("I'm a DecillionAI User"), false, false),
    );
    user_extender.insert(
        "location".to_string(),
        make_field("location", serde_json::json!("DecillionAI Land"), false, false),
    );
    let mut store_extender: HashMap<String, ExtendedField> = HashMap::new();
    store_extender.insert(
        "title".to_string(),
        make_field("title", serde_json::json!("Untitled Store"), true, true),
    );
    store_extender.insert(
        "avatar".to_string(),
        make_field("avatar", serde_json::json!("avatar"), false, true),
    );
    let mut model_extender: HashMap<String, HashMap<String, ExtendedField>> = HashMap::new();
    model_extender.insert("user".to_string(), user_extender);
    model_extender.insert("store".to_string(), store_extender);

    let app_for_plug: Arc<dyn crate::models::core::ICore> = app.clone();
    plug_all(app_for_plug, &model_extender);

    // ── Startup VMM listener restore ──────────────────────────────────────────
    // The signaler listeners that vmm.assign() registers are in-memory only.
    // After a node restart they are gone, so creature signals to deployed
    // machines would silently drop.  Scan the DB for all previously deployed
    // entity type links and re-register one listener per unique machine_id.
    {
        use std::collections::HashSet;
        let keys_slot = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let keys_clone = keys_slot.clone();
        app.modify_state(
            true,
            Box::new(move |trx: &dyn crate::models::transaction::ITrx| {
                *keys_clone.lock().unwrap() = trx.get_by_prefix("link::vmEntityType::");
                Ok(())
            }),
        );
        let mut seen: HashSet<String> = HashSet::new();
        for key in keys_slot.lock().unwrap().iter() {
            // key = "link::vmEntityType::MACHINE_ID::ENTITY_ID"
            let rest = key
                .strip_prefix("link::vmEntityType::")
                .unwrap_or("");
            if let Some(machine_id) = rest.split("::").next() {
                if !machine_id.is_empty() && seen.insert(machine_id.to_string()) {
                    app.tools().vmm().assign(machine_id);
                }
            }
        }
        if !seen.is_empty() {
            eprintln!(
                "[startup] Restored VMM listeners for {} machine(s): {:?}",
                seen.len(),
                seen
            );
        }
    }

    app.run();

    // ── Geo-distributed cluster mesh ──────────────────────────────────────────
    // When cluster mode is enabled (cluster.json / CLUSTER_* env), this node
    // joins the OpenRaft mesh of same-origin instances: shell API state and
    // distributed-mode creature deployments replicate to every instance, and
    // the cluster HTTP listener serves the raft RPC + `casparctl cluster`
    // orchestration API. Standalone nodes skip this entirely.
    {
        let app_for_cluster: Arc<dyn crate::models::core::ICore> = app.clone();
        drivers::cluster::init_from_env(app_for_cluster);
    }

    // ── Docker-host bridge gateway ────────────────────────────────────────────
    // Long-lived TCP server that docker-based creature containers connect to.
    // It is their only channel to the outside world: every host interaction
    // (DB/storage ops, outbound HTTP, signalling) and every inbound signal flows
    // over it. Disabled when the port is unset/zero.
    let docker_gateway_port: i64 = env::var("DOCKER_HOST_GATEWAY_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8079);
    app.tools().vmm().start_docker_gateway(docker_gateway_port);

    let port_tcp: i64 = env::var("CLIENT_TCP_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let port_ws: i64 = env::var("CLIENT_WS_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let port_fed: i64 = env::var("FEDERATION_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let port_chain: i64 = env::var("BLOCKCHAIN_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let mut ports: HashMap<String, i64> = HashMap::new();
    ports.insert("tcp".to_string(), port_tcp);
    ports.insert("ws".to_string(), port_ws);
    ports.insert("fed".to_string(), port_fed);
    ports.insert("chain".to_string(), port_chain);
    app.tools().network().run(ports);

    // Block forever — background threads run the gossip / chain dispatch.
    loop {
        thread::sleep(Duration::from_secs(60 * 60));
    }
}

fn parse_owner_key(pem: &str) -> Option<rsa::RsaPrivateKey> {
    use rsa::pkcs8::DecodePrivateKey;
    rsa::RsaPrivateKey::from_pkcs8_pem(pem).ok()
}

fn load_dotenv(path: &str) -> std::io::Result<()> {
    let content = std::fs::read_to_string(path)?;
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim();
            // Handle multi-line double-quoted values (e.g. PEM keys).
            let final_val: String = if v.starts_with('"') && !v[1..].contains('"') {
                // Opening quote but no closing quote on this line — accumulate
                // continuation lines until we find one that ends with '"'.
                let mut val = v[1..].to_string();
                loop {
                    match lines.next() {
                        Some(next) => {
                            val.push('\n');
                            val.push_str(next);
                            if next.ends_with('"') {
                                val.truncate(val.len() - 1);
                                break;
                            }
                        }
                        None => break,
                    }
                }
                val
            } else {
                v.trim_start_matches('"')
                    .trim_end_matches('"')
                    .trim_start_matches('\'')
                    .trim_end_matches('\'')
                    .to_string()
            };
            // SAFETY: setenv is unsafe in multi-threaded contexts but this
            // runs before any other thread is spawned.
            unsafe {
                env::set_var(k, &final_val);
            }
        }
    }
    Ok(())
}

fn install_signal_handler<F: FnOnce() + Send + 'static>(callback: F) {
    // Minimal SIGINT / SIGTERM handling without signal-hook: spawn a thread
    // that masks the signals and waits via `libc::sigwait`. Linux only —
    // matches the Caspar deployment target.
    thread::spawn(move || {
        #[cfg(target_os = "linux")]
        unsafe {
            let mut mask: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGINT);
            libc::sigaddset(&mut mask, libc::SIGTERM);
            libc::pthread_sigmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut());
            let mut sig: libc::c_int = 0;
            libc::sigwait(&mask, &mut sig);
        }
        callback();
    });
}
