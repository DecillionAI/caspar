//! Translation of `abstract/adapters/tools/tools.go`.

use std::sync::Arc;

use crate::abstractions::ports::file::IFile;
use crate::abstractions::ports::network::INetwork;
use crate::abstractions::ports::security::ISecurity;
use crate::abstractions::ports::signaler::ISignaler;
use crate::abstractions::ports::storage::IStorage;
use crate::abstractions::ports::vmm::IVmm;

/// Aggregates every node driver behind a single interface.
pub trait ITools: Send + Sync {
    fn security(&self) -> Arc<dyn ISecurity>;
    fn signaler(&self) -> Arc<dyn ISignaler>;
    fn storage(&self) -> Arc<dyn IStorage>;
    fn network(&self) -> Arc<dyn INetwork>;
    fn file(&self) -> Arc<dyn IFile>;
    fn vmm(&self) -> Arc<dyn IVmm>;
}
