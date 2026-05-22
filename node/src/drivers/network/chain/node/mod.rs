//! Translation of `chain/node` — the Babble consensus node.
//!
//! This part translates the node's support layer: the state machine, the
//! validator, the peer selector, the join promise and the control timer. The
//! consensus `Core`, the `Node` gossip loop and the RPC handlers follow next.

pub mod control_timer;
pub mod peer_selector;
pub mod promise;
pub mod state;
pub mod validator;

pub use control_timer::ControlTimer;
pub use peer_selector::{PeerSelector, RandomPeerSelector};
pub use promise::{JoinPromise, JoinPromiseResponse};
pub use state::{Manager, State, WGLIMIT};
pub use validator::Validator;
