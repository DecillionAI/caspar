//! Translation of `chain/config/config.go`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use k256::ecdsa::SigningKey;

use crate::drivers::network::chain::proxy::AppProxy;
use crate::logrus::{Entry, Level, Logger};

/// Default name of the file containing the validator's private key.
pub const DEFAULT_KEYFILE: &str = "priv_key";
/// Default name of the folder containing the database.
pub const DEFAULT_DB_FILE: &str = "rocksdb_db";
/// Default name of the signal-server TLS certificate file.
pub const DEFAULT_CERT_FILE: &str = "cert.pem";

pub const DEFAULT_LOG_LEVEL: &str = "debug";
pub const DEFAULT_BIND_ADDR: &str = "0.0.0.0:1337";
pub const DEFAULT_SERVICE_ADDR: &str = "0.0.0.0:8000";
pub const DEFAULT_CACHE_SIZE: i64 = 100;
pub const DEFAULT_SYNC_LIMIT: i64 = 1000;
pub const DEFAULT_MAX_POOL: i64 = 2;
pub const DEFAULT_STORE: bool = true;
pub const DEFAULT_MAINTENANCE_MODE: bool = false;
pub const DEFAULT_SUSPEND_LIMIT: i64 = 100;
pub const DEFAULT_WEBRTC: bool = false;
pub const DEFAULT_SIGNAL_ADDR: &str = "127.0.0.1:2443";
pub const DEFAULT_SIGNAL_REALM: &str = "main";
pub const DEFAULT_SIGNAL_SKIP_VERIFY: bool = false;
pub const DEFAULT_ICE_ADDRESS: &str = "stun:stun.l.google.com:19302";
pub const DEFAULT_ICE_USERNAME: &str = "";
pub const DEFAULT_ICE_PASSWORD: &str = "";

fn default_heartbeat_timeout() -> Duration {
    Duration::from_millis(10)
}
fn default_slow_heartbeat_timeout() -> Duration {
    Duration::from_millis(1000)
}
fn default_tcp_timeout() -> Duration {
    Duration::from_millis(1000)
}
fn default_join_timeout() -> Duration {
    Duration::from_millis(10000)
}

/// A server providing ICE services (STUN/TURN), the translation of
/// `webrtc.ICEServer`.
#[derive(Debug, Clone, Default)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
    pub credential_type: String,
}

/// All the configuration properties of a Babble node.
#[derive(Clone)]
pub struct Config {
    pub data_dir: String,
    pub log_level: String,
    pub bind_addr: String,
    pub advertise_addr: String,
    pub no_service: bool,
    pub service_addr: String,
    pub heartbeat_timeout: Duration,
    pub slow_heartbeat_timeout: Duration,
    pub max_pool: i64,
    pub tcp_timeout: Duration,
    pub join_timeout: Duration,
    pub sync_limit: i64,
    pub enable_fast_sync: bool,
    pub store: bool,
    pub database_dir: String,
    pub cache_size: i64,
    pub bootstrap: bool,
    pub maintenance_mode: bool,
    pub suspend_limit: i64,
    pub moniker: String,
    pub webrtc: bool,
    pub signal_addr: String,
    pub signal_realm: String,
    pub signal_skip_verify: bool,
    pub ice_address: String,
    pub ice_username: String,
    pub ice_password: String,
    /// The application proxy that lets Babble communicate with the app.
    pub proxy: Option<Arc<dyn AppProxy>>,
    /// The private key of the validator.
    pub key: Option<SigningKey>,
}

impl Config {
    /// Returns a config object with default values.
    pub fn new_default_config(adv_addr: &str) -> Config {
        Config {
            advertise_addr: adv_addr.to_string(),
            data_dir: default_data_dir(),
            log_level: DEFAULT_LOG_LEVEL.to_string(),
            bind_addr: DEFAULT_BIND_ADDR.to_string(),
            service_addr: DEFAULT_SERVICE_ADDR.to_string(),
            heartbeat_timeout: default_heartbeat_timeout(),
            slow_heartbeat_timeout: default_slow_heartbeat_timeout(),
            tcp_timeout: default_tcp_timeout(),
            join_timeout: default_join_timeout(),
            cache_size: DEFAULT_CACHE_SIZE,
            sync_limit: DEFAULT_SYNC_LIMIT,
            max_pool: DEFAULT_MAX_POOL,
            store: DEFAULT_STORE,
            maintenance_mode: DEFAULT_MAINTENANCE_MODE,
            database_dir: default_database_dir(),
            suspend_limit: DEFAULT_SUSPEND_LIMIT,
            webrtc: DEFAULT_WEBRTC,
            signal_addr: DEFAULT_SIGNAL_ADDR.to_string(),
            signal_realm: DEFAULT_SIGNAL_REALM.to_string(),
            signal_skip_verify: DEFAULT_SIGNAL_SKIP_VERIFY,
            ice_address: DEFAULT_ICE_ADDRESS.to_string(),
            ice_username: DEFAULT_ICE_USERNAME.to_string(),
            ice_password: DEFAULT_ICE_PASSWORD.to_string(),
            no_service: false,
            enable_fast_sync: false,
            bootstrap: false,
            moniker: String::new(),
            proxy: None,
            key: None,
        }
    }

    /// Sets the top-level Babble directory, updating the database directory if
    /// it is still at its default value.
    pub fn set_data_dir(&mut self, data_dir: &str) {
        self.data_dir = data_dir.to_string();
        if self.database_dir == default_database_dir() {
            self.database_dir = PathBuf::from(data_dir)
                .join(DEFAULT_DB_FILE)
                .to_string_lossy()
                .into_owned();
        }
    }

    /// The full path of the file containing the private key.
    pub fn keyfile(&self) -> String {
        PathBuf::from(&self.data_dir)
            .join(DEFAULT_KEYFILE)
            .to_string_lossy()
            .into_owned()
    }

    /// The full path of the signal-server TLS certificate file.
    pub fn cert_file(&self) -> String {
        PathBuf::from(&self.data_dir)
            .join(DEFAULT_CERT_FILE)
            .to_string_lossy()
            .into_owned()
    }

    /// The list of ICE servers used by the WebRTC stream layer.
    pub fn ice_servers(&self) -> Vec<IceServer> {
        vec![IceServer {
            urls: vec![self.ice_address.clone()],
            username: self.ice_username.clone(),
            credential: self.ice_password.clone(),
            credential_type: "password".to_string(),
        }]
    }

    /// Returns a formatted logrus `Entry` with prefix "babble".
    pub fn logger(&self) -> Entry {
        let logger = Logger::new();
        logger.set_level(log_level(&self.log_level));
        Entry::new(logger).with_field("prefix", "babble")
    }
}

/// The default path for the database files.
pub fn default_database_dir() -> String {
    PathBuf::from(default_data_dir())
        .join(DEFAULT_DB_FILE)
        .to_string_lossy()
        .into_owned()
}

/// The default directory for top-level Babble config, based on the OS.
pub fn default_data_dir() -> String {
    let home = home_dir();
    if !home.is_empty() {
        let p = match std::env::consts::OS {
            "macos" => PathBuf::from(&home).join(".Babble"),
            "windows" => PathBuf::from(&home).join("AppData").join("Roaming").join("Babble"),
            _ => PathBuf::from(&home).join(".babble"),
        };
        return p.to_string_lossy().into_owned();
    }
    String::new()
}

/// The user's home directory.
pub fn home_dir() -> String {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return home;
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.is_empty() {
            return profile;
        }
    }
    String::new()
}

/// Parses a string into a logrus log level.
pub fn log_level(l: &str) -> Level {
    match l {
        "debug" => Level::Debug,
        "info" => Level::Info,
        "warn" => Level::Warn,
        "error" => Level::Error,
        "fatal" => Level::Fatal,
        "panic" => Level::Panic,
        _ => Level::Debug,
    }
}

/// The default ICE configuration, with one public Google STUN server.
pub fn default_ice_servers() -> Vec<IceServer> {
    vec![IceServer {
        urls: vec![DEFAULT_ICE_ADDRESS.to_string()],
        ..IceServer::default()
    }]
}
