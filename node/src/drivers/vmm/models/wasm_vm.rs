use crate::drivers::vmm::prelude::*;
use crate::drivers::vmm::models::base_vm::BaseVm;

#[derive(Clone, Debug)]
pub(crate) struct WasmVm {
    pub(crate) base: BaseVm,
    pub(crate) module_path: String,
    pub(crate) entry_point: String,
}
