//! Translation of `chain/node/state/state.go`.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;

/// Captures the state of a Babble node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum State {
    /// Gossips regularly as part of the hashgraph consensus algorithm.
    Babbling = 0,
    /// Attempts to fast-forward to a future store (FastSync).
    CatchingUp = 1,
    /// Attempts to join a Babble group by submitting a join request.
    Joining = 2,
    /// Attempts to politely leave a Babble group.
    Leaving = 3,
    /// Stops responding to external events and closes its transport.
    Shutdown = 4,
    /// Passively participates in gossip but processes no new events.
    Suspended = 5,
}

impl Default for State {
    fn default() -> Self {
        State::Babbling
    }
}

impl State {
    fn from_u32(v: u32) -> State {
        match v {
            1 => State::CatchingUp,
            2 => State::Joining,
            3 => State::Leaving,
            4 => State::Shutdown,
            5 => State::Suspended,
            _ => State::Babbling,
        }
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            State::Babbling => "Babbling",
            State::CatchingUp => "CatchingUp",
            State::Joining => "Joining",
            State::Leaving => "Leaving",
            State::Shutdown => "Shutdown",
            State::Suspended => "Suspended",
        };
        write!(f, "{}", s)
    }
}

/// The maximum number of background tasks that can be launched through
/// [`Manager::go_func`].
pub const WGLIMIT: i32 = 20;

/// Wraps a [`State`] with get/set methods. It also limits the number of
/// background tasks the node launches, and waits for all of them to complete.
pub struct Manager {
    state: AtomicU32,
    wg_count: AtomicI32,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl Default for Manager {
    fn default() -> Self {
        Manager::new()
    }
}

impl Manager {
    pub fn new() -> Manager {
        Manager {
            state: AtomicU32::new(State::Babbling as u32),
            wg_count: AtomicI32::new(0),
            handles: Mutex::new(Vec::new()),
        }
    }

    /// Returns the current state.
    pub fn get_state(&self) -> State {
        State::from_u32(self.state.load(Ordering::SeqCst))
    }

    /// Sets the state.
    pub fn set_state(&self, s: State) {
        self.state.store(s as u32, Ordering::SeqCst);
    }

    /// Launches a background task for `f`, if fewer than [`WGLIMIT`] are
    /// currently outstanding.
    pub fn go_func(&self, f: Box<dyn FnOnce() + Send>) {
        if self.wg_count.load(Ordering::SeqCst) < WGLIMIT {
            self.wg_count.fetch_add(1, Ordering::SeqCst);
            let handle = std::thread::spawn(move || f());
            self.handles.lock().unwrap().push(handle);
        }
    }

    /// Waits for all background tasks to complete.
    pub fn wait_routines(&self) {
        let handles: Vec<JoinHandle<()>> =
            std::mem::take(&mut *self.handles.lock().unwrap());
        for h in handles {
            let _ = h.join();
        }
        self.wg_count.store(0, Ordering::SeqCst);
    }
}
