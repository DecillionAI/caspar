use crate::drivers::vmm::appengine::prelude::*;
use crate::drivers::vmm::appengine::models::base_vm::BaseVm;

#[derive(Clone, Debug)]
pub(crate) struct ElpifyVm {
    pub(crate) base: BaseVm,
    pub(crate) masm_path: String,
}
