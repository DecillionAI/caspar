use std::sync::Arc;

use anyhow::Result;
use serde_json::{Map, Value};

use crate::models::input::IInput;
use crate::models::transaction::ITrx;
use crate::models::state::IState;
use crate::util::AnyVal;

/// Closure handed to a state modifier — operates on an [`ITrx`].
pub type TrxClosure = Box<dyn FnMut(&dyn ITrx) -> Result<()> + Send>;

/// Function used to schedule a state modification. The `bool` is the
/// readonly flag; the closure runs against the opened transaction.
pub type StateModifierFn = Box<dyn Fn(bool, TrxClosure) + Send + Sync>;

/// A collection of actions that can be installed onto a state machine.
pub trait IActions: Send + Sync {
    fn install(&self, state: Arc<dyn IState>, args: Vec<AnyVal>);
}

/// A single action.
pub trait IAction: Send + Sync {
    fn state_modifier(&self) -> StateModifierFn;
    fn key(&self) -> String;
    fn act(&self, state: Arc<dyn IState>, input: Arc<dyn IInput>) -> Result<(i64, Value)>;
}

/// An action that performs its own authentication/authorization.
pub trait ISecureAction: Send + Sync {
    fn key(&self) -> String;
    fn has_global_parser(&self) -> bool;
    fn parse_input(&self, protocol: &str, raw: Value) -> Result<Arc<dyn IInput>>;
    #[allow(clippy::too_many_arguments)]
    fn securely_act(
        &self,
        user_id: &str,
        packet_id: &str,
        packet_binary: &[u8],
        packet_signature: &str,
        input: Arc<dyn IInput>,
        dummy: &str,
        insider: &[bool],
    ) -> Result<(i64, Value)>;
    #[allow(clippy::too_many_arguments)]
    fn securly_act_chain(
        &self,
        user_id: &str,
        packet_id: &str,
        packet_binary: &[u8],
        packet_signature: &str,
        input: Arc<dyn IInput>,
        origin: &str,
        tag: &str,
    ) -> Result<(i64, Value)>;
    fn securely_act_fed(
        &self,
        user_id: &str,
        packet_binary: &[u8],
        packet_signature: &str,
        input: Arc<dyn IInput>,
    ) -> Result<(i64, Value)>;
}

/// Describes a dynamically pluggable field on an entity (user, store, ...).
#[derive(Clone, Default)]
pub struct ExtendedField {
    pub name: String,
    pub path: String,
    pub typ: String,
    pub default: Value,
    pub required: bool,
    pub searchable: bool,
    pub primary_prop: bool,
    pub get_value:
        Option<Arc<dyn Fn(Arc<dyn IState>, Map<String, Value>) -> Result<Value> + Send + Sync>>,
}
