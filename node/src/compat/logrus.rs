//! A minimal translation shim for `github.com/sirupsen/logrus`.
//!
//! The vendored Babble hashgraph/node code logs through logrus
//! (`*logrus.Entry`, `*logrus.Logger`, `logrus.FieldLogger`). This module
//! reproduces the parts of the API the node uses: levelled logging plus the
//! `WithField` / `WithError` / `WithFields` structured-field builders.

use std::fmt::Display;
use std::sync::{Arc, RwLock};

/// Mirrors `logrus.Level`. Numeric order matches logrus exactly (lower is more
/// severe), so `logger.level >= msg.level` decides whether a line is emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Panic = 0,
    Fatal = 1,
    Error = 2,
    Warn = 3,
    Info = 4,
    Debug = 5,
    Trace = 6,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Panic => "panic",
            Level::Fatal => "fatal",
            Level::Error => "error",
            Level::Warn => "warning",
            Level::Info => "info",
            Level::Debug => "debug",
            Level::Trace => "trace",
        }
    }
}

/// Mirrors `logrus.Logger` — essentially just a shared, mutable level.
#[derive(Debug)]
pub struct Logger {
    level: RwLock<Level>,
}

impl Logger {
    /// Equivalent of `logrus.New()`.
    pub fn new() -> Arc<Logger> {
        Arc::new(Logger { level: RwLock::new(Level::Info) })
    }

    pub fn set_level(&self, level: Level) {
        *self.level.write().unwrap() = level;
    }

    pub fn level(&self) -> Level {
        *self.level.read().unwrap()
    }
}

impl Default for Logger {
    fn default() -> Self {
        Logger { level: RwLock::new(Level::Info) }
    }
}

/// Mirrors `logrus.Entry` — a logger plus a set of accumulated fields.
///
/// Cloning an `Entry` is cheap (it shares the underlying `Logger`).
#[derive(Clone)]
pub struct Entry {
    logger: Arc<Logger>,
    fields: Vec<(String, String)>,
}

impl Entry {
    /// Equivalent of `logrus.NewEntry(logger)`.
    pub fn new(logger: Arc<Logger>) -> Entry {
        Entry { logger, fields: Vec::new() }
    }

    /// Convenience constructor producing a brand new logger + entry.
    pub fn standalone() -> Entry {
        Entry::new(Logger::new())
    }

    pub fn logger(&self) -> Arc<Logger> {
        self.logger.clone()
    }

    /// `WithField(key, value)`.
    pub fn with_field(&self, key: &str, value: impl Display) -> Entry {
        let mut fields = self.fields.clone();
        fields.push((key.to_string(), value.to_string()));
        Entry { logger: self.logger.clone(), fields }
    }

    /// `WithFields(logrus.Fields{...})`.
    pub fn with_fields(&self, kvs: &[(&str, String)]) -> Entry {
        let mut fields = self.fields.clone();
        for (k, v) in kvs {
            fields.push((k.to_string(), v.clone()));
        }
        Entry { logger: self.logger.clone(), fields }
    }

    /// `WithError(err)`.
    pub fn with_error(&self, err: impl Display) -> Entry {
        self.with_field("error", err)
    }

    fn emit(&self, level: Level, msg: &str) {
        if self.logger.level() < level {
            return;
        }
        let mut line = format!(
            "{} [{}] {}",
            crate::compat::golog::now_prefix(),
            level.label(),
            msg
        );
        for (k, v) in &self.fields {
            line.push_str(&format!(" {}={}", k, v));
        }
        eprintln!("{}", line);
    }

    pub fn trace(&self, msg: impl Display) {
        self.emit(Level::Trace, &msg.to_string());
    }
    pub fn debug(&self, msg: impl Display) {
        self.emit(Level::Debug, &msg.to_string());
    }
    pub fn info(&self, msg: impl Display) {
        self.emit(Level::Info, &msg.to_string());
    }
    pub fn warn(&self, msg: impl Display) {
        self.emit(Level::Warn, &msg.to_string());
    }
    pub fn error(&self, msg: impl Display) {
        self.emit(Level::Error, &msg.to_string());
    }
    pub fn fatal(&self, msg: impl Display) {
        self.emit(Level::Fatal, &msg.to_string());
        std::process::exit(1);
    }
    pub fn panic(&self, msg: impl Display) {
        let s = msg.to_string();
        self.emit(Level::Panic, &s);
        panic!("{}", s);
    }
}

impl Default for Entry {
    fn default() -> Self {
        Entry::standalone()
    }
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "logrus::Entry({} fields)", self.fields.len())
    }
}
