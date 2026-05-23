//! Thin compatibility layer that exposes appengine bootstrap APIs from
//! the parent `vmm` module. This mirrors the historical Go layout where
//! appengine bootstrap is triggered from the core runtime `run` stage.

use std::sync::Once;
use std::thread;

pub use crate::drivers::vmm::appengine::bootstrap::restore_previously_running_vms;

static APPENGINE_BOOTSTRAP: Once = Once::new();

/// Start embedded appengine bridge loops once per process.
pub fn run() {
    APPENGINE_BOOTSTRAP.call_once(|| {
        thread::spawn(|| {
            crate::drivers::vmm::appengine::bootstrap::run();
        });
    });
}
