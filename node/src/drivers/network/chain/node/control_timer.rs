//! Translation of `chain/node/control_timer.go`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{after, bounded, never, select, Receiver, Sender};

/// Produces a timer channel for a given duration.
pub type TimerFactory = Box<dyn Fn(Duration) -> Receiver<Instant> + Send + Sync>;

/// Controls the node's heartbeat.
pub struct ControlTimer {
    timer_factory: TimerFactory,
    /// Signals a tick to the listening process.
    pub tick_rx: Receiver<()>,
    tick_tx: Sender<()>,
    reset_tx: Sender<Duration>,
    reset_rx: Receiver<Duration>,
    stop_tx: Sender<()>,
    stop_rx: Receiver<()>,
    shutdown_tx: Sender<()>,
    shutdown_rx: Receiver<()>,
    is_set: AtomicBool,
    is_shutdown: AtomicBool,
}

impl ControlTimer {
    /// A `ControlTimer` factory method.
    pub fn new(timer_factory: TimerFactory) -> ControlTimer {
        let (tick_tx, tick_rx) = bounded(0);
        let (reset_tx, reset_rx) = bounded(0);
        let (stop_tx, stop_rx) = bounded(0);
        let (shutdown_tx, shutdown_rx) = bounded(0);
        ControlTimer {
            timer_factory,
            tick_rx,
            tick_tx,
            reset_tx,
            reset_rx,
            stop_tx,
            stop_rx,
            shutdown_tx,
            shutdown_rx,
            is_set: AtomicBool::new(false),
            is_shutdown: AtomicBool::new(false),
        }
    }

    /// Creates a `ControlTimer` that ticks at random intervals between `min`
    /// and `2*min`.
    pub fn new_random() -> ControlTimer {
        let random_timeout: TimerFactory = Box::new(|min: Duration| {
            if min.is_zero() {
                return never();
            }
            let min_nanos = min.as_nanos() as u64;
            let extra = (rand::random::<u64>() >> 1) % min_nanos.max(1);
            after(min + Duration::from_nanos(extra))
        });
        ControlTimer::new(random_timeout)
    }

    /// Starts the control timer. Intended to be run on its own thread.
    pub fn run(&self, init: Duration) {
        let set_timer = |t: Duration| -> Receiver<Instant> {
            self.is_set.store(true, Ordering::SeqCst);
            (self.timer_factory)(t)
        };

        let mut timer = set_timer(init);
        loop {
            select! {
                recv(timer) -> result => {
                    // Only deliver a tick when the timer actually
                    // fired. Without this guard, an exhausted `after()`
                    // channel becomes always-ready with
                    // `Err(RecvError)` and the select arm fires every
                    // iteration: a fake tick is pushed into the
                    // bounded(0) `tick_tx`, which blocks waiting for
                    // the babble loop to consume it. The babble loop,
                    // having processed the previous tick, is sitting
                    // in `reset_timer()` trying to send the next
                    // duration into the bounded(0) `reset_tx`, which
                    // requires this timer thread to be at `recv(reset_rx)`
                    // — but we are blocked in `tick_tx.send`. The two
                    // bounded-0 channels rendezvous on each other and
                    // the whole control plane stalls. Switching `timer`
                    // to `never()` after firing makes this loop wait
                    // for an explicit `reset_rx` before producing the
                    // next tick, which mirrors Go's `time.Timer` Reset
                    // semantics that the original code translated from.
                    if result.is_ok() {
                        let _ = self.tick_tx.send(());
                    }
                    self.is_set.store(false, Ordering::SeqCst);
                    timer = never();
                }
                recv(self.reset_rx) -> t => {
                    if let Ok(t) = t {
                        timer = set_timer(t);
                    }
                }
                recv(self.stop_rx) -> _ => {
                    timer = never();
                    self.is_set.store(false, Ordering::SeqCst);
                }
                recv(self.shutdown_rx) -> _ => {
                    self.is_set.store(false, Ordering::SeqCst);
                    return;
                }
            }
        }
    }

    /// Resets the heartbeat timer to a new duration.
    pub fn reset(&self, t: Duration) {
        let _ = self.reset_tx.send(t);
    }

    /// Stops the heartbeat timer.
    pub fn stop(&self) {
        let _ = self.stop_tx.send(());
    }

    /// Returns whether the timer is currently set.
    pub fn is_set(&self) -> bool {
        self.is_set.load(Ordering::SeqCst)
    }

    /// Shuts down the control timer.
    pub fn shutdown(&self) {
        if !self.is_shutdown.swap(true, Ordering::SeqCst) {
            let _ = self.shutdown_tx.send(());
        }
    }
}
