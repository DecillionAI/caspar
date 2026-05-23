use crate::drivers::vmm::prelude::*;
use crate::drivers::vmm::models::base_vm::BaseVm;

#[derive(Clone, Debug)]
pub(crate) struct ElpifyVm {
    pub(crate) base: BaseVm,
    pub(crate) masm_path: String,
}
