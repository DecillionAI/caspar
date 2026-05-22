//! Translation of `abstract/state/state.go`.

use std::sync::Arc;

use crate::abstractions::models::info::IInfo;
use crate::abstractions::models::trx::ITrx;

/// State carried through a secured state modification.
pub trait IState: Send + Sync {
    fn info(&self) -> Arc<dyn IInfo>;
    fn trx(&self) -> Arc<dyn ITrx>;
    fn set_trx(&self, trx: Arc<dyn ITrx>);
    fn source(&self) -> String;
    fn set_source(&self, source: &str);
}
