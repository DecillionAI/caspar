use std::sync::Arc;

use crate::models::ports::file::IFile;
use crate::models::ports::network::INetwork;
use crate::models::ports::security::ISecurity;
use crate::models::ports::signaler::ISignaler;
use crate::models::ports::storage::IStorage;
use crate::models::ports::vmm::IVmm;

/// Aggregates every node driver behind a single interface.
pub trait ITools: Send + Sync {
    fn security(&self) -> Arc<dyn ISecurity>;
    fn signaler(&self) -> Arc<dyn ISignaler>;
    fn storage(&self) -> Arc<dyn IStorage>;
    fn network(&self) -> Arc<dyn INetwork>;
    fn file(&self) -> Arc<dyn IFile>;
    fn vmm(&self) -> Arc<dyn IVmm>;
}
