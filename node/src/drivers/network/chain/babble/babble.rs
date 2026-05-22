//! Translation of `chain/babble/babble.go`.
//!
//! `Babble` is the top-level engine that glues a `Config`, `Store`,
//! `Transport`, `Proxy` and `Node` together. The Go implementation also wired
//! in a WebRTC/WAMP transport and a debug HTTP `service.Service`; both are
//! intentionally dropped in the Rust port — the only network transport offered
//! is the custom-TCP `NetworkTransport` from `chain::net`, and the debug HTTP
//! service is not translated (the node's stats are available directly through
//! `Node` accessors).

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};

use crate::drivers::network::chain::config::Config;
use crate::drivers::network::chain::crypto::keys::{KeyReaderWriter, SimpleKeyfile};
use crate::drivers::network::chain::hashgraph::{InmemStore, RocksDbStore, Store};
use crate::drivers::network::chain::net::{new_tcp_transport, Transport};
use crate::drivers::network::chain::node::{Node, Validator};
use crate::drivers::network::chain::peers::{JSONPeerSet, PeerSet};
use crate::logrus::Entry;

/// Encapsulates the components that make up a Babble node.
pub struct Babble {
    pub config: Arc<Config>,
    pub node: Option<Arc<Node>>,
    pub transport: Option<Arc<dyn Transport>>,
    pub store: Option<Box<dyn Store>>,
    pub peers: Option<Arc<PeerSet>>,
    pub genesis_peers: Option<Arc<PeerSet>>,
    pub logger: Entry,
}

impl Babble {
    /// Returns a new `Babble` engine.
    pub fn new(config: Arc<Config>) -> Babble {
        let logger = config.logger();
        Babble {
            config,
            node: None,
            transport: None,
            store: None,
            peers: None,
            genesis_peers: None,
            logger,
        }
    }

    /// Initialises the engine in dependency order: config validation, key,
    /// peers, store, transport, node. The `transport` argument lets the caller
    /// inject an out-of-band transport (used by tests); pass `None` to build
    /// a TCP transport from the config.
    pub fn init(
        &mut self,
        transport: Option<Arc<dyn Transport>>,
        work_chain_id: &str,
        shard_chain_id: &str,
        on_new_node_cb: Option<Box<dyn Fn(String) + Send + Sync>>,
    ) -> Result<()> {
        self.logger.debug("validateConfig");
        if let Err(e) = self.validate_config() {
            self.logger
                .with_error(&e)
                .error("babble.rs:init() validate_config");
        }

        self.logger.debug("initKey");
        self.init_key()
            .map_err(|e| {
                self.logger
                    .with_error(&e)
                    .error("babble.rs:init() init_key");
                e
            })?;

        self.logger.debug("initPeers");
        self.init_peers().map_err(|e| {
            self.logger
                .with_error(&e)
                .error("babble.rs:init() init_peers");
            e
        })?;

        self.logger.debug("initStore");
        self.init_store().map_err(|e| {
            self.logger
                .with_error(&e)
                .error("babble.rs:init() init_store");
            e
        })?;

        self.logger.debug("initTransport");
        if let Some(t) = transport {
            self.transport = Some(t);
        } else if !self.config.maintenance_mode {
            self.init_transport().map_err(|e| {
                self.logger
                    .with_error(&e)
                    .error("babble.rs:init() init_transport");
                e
            })?;
        }

        self.logger.debug("initNode");
        self.init_node(work_chain_id, shard_chain_id, on_new_node_cb)
            .map_err(|e| {
                self.logger
                    .with_error(&e)
                    .error("babble.rs:init() init_node");
                e
            })?;

        Ok(())
    }

    /// Starts the Babble node — runs the gossip loop.
    pub fn run(&self) {
        if let Some(n) = &self.node {
            n.run(true);
        }
    }

    fn validate_config(&self) -> Result<()> {
        // Note: SetDataDir would mutate the config; the Rust translation keeps
        // Config behind an `Arc` so callers configure it before constructing
        // the engine. The remaining validation only logs.

        // Maintenance-mode only works with bootstrap; bootstrap only works
        // with store. We can't mutate the shared `Arc<Config>` here, so we
        // surface this as a hard error if the combination is wrong.
        if self.config.maintenance_mode && !self.config.bootstrap {
            return Err(anyhow!(
                "maintenance-mode requires bootstrap to be enabled"
            ));
        }
        if self.config.bootstrap && !self.config.store {
            return Err(anyhow!("bootstrap requires store to be enabled"));
        }
        if self.config.slow_heartbeat_timeout < self.config.heartbeat_timeout {
            self.logger.debug(format!(
                "SlowHeartbeatTimeout ({:?}) cannot be less than Heartbeat ({:?})",
                self.config.slow_heartbeat_timeout, self.config.heartbeat_timeout
            ));
            // Caller can fix this by constructing the config correctly; we
            // don't try to mutate `Arc<Config>` from here.
        }
        Ok(())
    }

    fn init_transport(&mut self) -> Result<()> {
        // WebRTC support is dropped; only TCP is offered.
        let timeout = self.config.tcp_timeout;
        let join_timeout = self.config.join_timeout;
        let max_pool = if self.config.max_pool <= 0 {
            1
        } else {
            self.config.max_pool as usize
        };
        let trans = new_tcp_transport(
            &self.config.bind_addr,
            &self.config.advertise_addr,
            max_pool,
            timeout,
            join_timeout,
            Some(self.logger.clone()),
        )?;
        self.transport = Some(trans);
        Ok(())
    }

    fn init_peers(&mut self) -> Result<()> {
        let peer_store = JSONPeerSet::new(&self.config.data_dir, true);
        let participants = peer_store
            .peer_set()?
            .ok_or_else(|| anyhow!("peers.json: no participants found"))?;
        let participants = Arc::new(participants);
        self.peers = Some(participants.clone());

        // Genesis peer-set is optional — fall back to the current set.
        let genesis_store = JSONPeerSet::new(&self.config.data_dir, false);
        let genesis = match genesis_store.peer_set() {
            Ok(Some(g)) => Arc::new(g),
            _ => {
                self.logger
                    .debug("could not read peers.genesis.json; using current set");
                participants
            }
        };
        self.genesis_peers = Some(genesis);
        Ok(())
    }

    fn init_store(&mut self) -> Result<()> {
        if !self.config.store {
            self.logger.debug("Creating InmemStore");
            self.store = Some(Box::new(InmemStore::new(self.config.cache_size)));
        } else {
            let db_path = &self.config.database_dir;
            self.logger
                .with_field("path", db_path)
                .debug("Creating RocksDB store");

            if !self.config.bootstrap {
                let backup = backup_file_name(db_path);
                match fs::rename(db_path, &backup) {
                    Ok(()) => self
                        .logger
                        .with_field("path", &backup)
                        .debug("Created backup"),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        self.logger.debug("Nothing to backup");
                    }
                    Err(e) => return Err(anyhow!("backup db dir: {}", e)),
                }
            }

            let store = RocksDbStore::new(
                self.config.cache_size,
                db_path,
                self.config.maintenance_mode,
            )?;
            self.store = Some(Box::new(store));
        }
        Ok(())
    }

    fn init_key(&mut self) -> Result<()> {
        if self.config.key.is_some() {
            return Ok(());
        }
        // We can't mutate `Arc<Config>`. Callers that don't pre-load the key
        // must pass a `Config` with `key: Some(...)`, or rely on
        // `load_key_for_config` below to construct one before wrapping it in
        // the Arc.
        Err(anyhow!(
            "Config.key is unset; pre-load the validator key with \
             `babble::load_key_for_config` before constructing Babble"
        ))
    }

    fn init_node(
        &mut self,
        work_chain_id: &str,
        shard_chain_id: &str,
        on_new_node_cb: Option<Box<dyn Fn(String) + Send + Sync>>,
    ) -> Result<()> {
        let key = self
            .config
            .key
            .clone()
            .ok_or_else(|| anyhow!("Config.key is unset"))?;
        let mut validator = Validator::new(key, &self.config.moniker);

        // Borrow moniker override from peers.json, matching Go behaviour.
        if let Some(peers) = &self.peers {
            if let Some(p) = peers.by_id.get(&validator.id()) {
                if p.moniker != validator.moniker {
                    self.logger.debug(format!(
                        "Using moniker `{}` from peers.json (was `{}`)",
                        p.moniker, validator.moniker
                    ));
                    validator.moniker = p.moniker.clone();
                }
            }
        }

        let peers = self
            .peers
            .clone()
            .ok_or_else(|| anyhow!("init_peers must run before init_node"))?;
        let genesis_peers = self
            .genesis_peers
            .clone()
            .ok_or_else(|| anyhow!("init_peers must run before init_node"))?;
        let store = self
            .store
            .take()
            .ok_or_else(|| anyhow!("init_store must run before init_node"))?;
        let transport = self
            .transport
            .clone()
            .ok_or_else(|| anyhow!("init_transport must run before init_node"))?;
        let proxy = self
            .config
            .proxy
            .clone()
            .ok_or_else(|| anyhow!("Config.proxy must be set before init_node"))?;

        let on_new_node: Box<dyn Fn(String) + Send + Sync> =
            on_new_node_cb.unwrap_or_else(|| Box::new(|_| {}));

        let node = Node::new(
            self.config.clone(),
            validator,
            peers,
            genesis_peers,
            store,
            transport,
            proxy,
            work_chain_id,
            shard_chain_id,
            on_new_node,
        );
        node.init()?;
        self.node = Some(node);
        Ok(())
    }
}

/// Loads the validator key from disk and writes it into `config.key`.
///
/// Use this before wrapping `Config` in `Arc` so the loaded key is visible to
/// the [`Babble`] engine via the shared `Arc<Config>`.
pub fn load_key_for_config(config: &mut Config) -> Result<()> {
    if config.key.is_some() {
        return Ok(());
    }
    let keyfile = SimpleKeyfile::new(&config.keyfile());
    let key = keyfile.read_key().map_err(|e| {
        anyhow!(
            "Error reading private key from file {}: {}",
            config.keyfile(),
            e
        )
    })?;
    config.key = Some(key);
    Ok(())
}

/// `<base>--UTC--<ts>` — matches Go's backup naming convention.
fn backup_file_name(base: &str) -> String {
    format!("{}--UTC--{}", base, to_iso8601(Utc::now()))
}

fn to_iso8601(t: DateTime<Utc>) -> String {
    use chrono::Datelike;
    use chrono::Timelike;
    format!(
        "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}.{:09}Z",
        t.year(),
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.nanosecond()
    )
}

// Unused-import suppression in case `Path` becomes needed later.
const _: fn() -> Option<&'static Path> = || None;
