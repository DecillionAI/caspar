use crate::drivers::vmm::prelude::*;
use crate::drivers::vmm::models::base_vm::BaseVm;

#[derive(Clone, Debug)]
pub(crate) struct ElpianVm {
    pub(crate) base: BaseVm,
    pub(crate) ast_path: String,
}
