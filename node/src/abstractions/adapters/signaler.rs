//! Translation of `abstract/adapters/signaler/signaler.go`.

use std::sync::Arc;

use dashmap::DashMap;
use serde_json::Value;

/// A signal callback: `func(string, any)`.
pub type SignalFn = Arc<dyn Fn(String, Value) + Send + Sync>;

/// A join/leave callback: `func(string, string)`.
pub type JoinFn = Arc<dyn Fn(String, String) + Send + Sync>;

/// A group of stores sharing a single listener.
#[derive(Clone)]
pub struct Group {
    pub stores: Arc<DashMap<String, String>>,
    pub listener: Arc<Listener>,
    pub override_: bool,
}

/// A single signal listener.
#[derive(Clone)]
pub struct Listener {
    pub id: String,
    pub paused: bool,
    pub dis_time: i64,
    pub signal: SignalFn,
}

/// A listener that bridges every signal globally.
#[derive(Clone)]
pub struct GlobalListener {
    pub signal: SignalFn,
}

/// A listener for group join/leave events.
#[derive(Clone)]
pub struct JoinListener {
    pub join: JoinFn,
    pub leave: JoinFn,
}

/// The signaler driver interface — realtime pub/sub fan-out.
pub trait ISignaler: Send + Sync {
    fn lock(&self);
    fn unlock(&self);
    fn listeners(&self) -> Arc<DashMap<String, Arc<Listener>>>;
    fn groups(&self) -> Arc<DashMap<String, Arc<Group>>>;
    fn listen_to_single(&self, listener: Arc<Listener>);
    fn listen_to_group(&self, listener: Arc<Listener>, override_functionaly: bool);
    fn brdige_globally(&self, listener: Arc<GlobalListener>, override_functionaly: bool);
    fn listen_to_join(&self, listener: Arc<JoinListener>);
    fn signal_user(&self, key: &str, listener_id: &str, data: Value, pack: bool);
    fn signal_group(
        &self,
        key: &str,
        group_id: &str,
        data: Value,
        pack: bool,
        exceptions: Vec<String>,
    );
    fn join_group(&self, group_id: &str, user_id: &str);
    fn leave_group(&self, group_id: &str, user_id: &str);
    fn retrive_group(&self, group_id: &str) -> Option<Arc<Group>>;
}
