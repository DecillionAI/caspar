//! Translation of `abstract/adapters/tools/tools.go`.

use std::sync::Arc;

use crate::abstractions::adapters::file::IFile;
use crate::abstractions::adapters::network::network::INetwork;
use crate::abstractions::adapters::security::ISecurity;
use crate::abstractions::adapters::signaler::ISignaler;
use crate::abstractions::adapters::storage::IStorage;
use crate::abstractions::adapters::vmm::IVmm;

/// Aggregates every node driver behind a single interface.
pub trait ITools: Send + Sync {
    fn security(&self) -> Arc<dyn ISecurity>;
    fn signaler(&self) -> Arc<dyn ISignaler>;
    fn storage(&self) -> Arc<dyn IStorage>;
    fn network(&self) -> Arc<dyn INetwork>;
    fn file(&self) -> Arc<dyn IFile>;
    fn vmm(&self) -> Arc<dyn IVmm>;
}
