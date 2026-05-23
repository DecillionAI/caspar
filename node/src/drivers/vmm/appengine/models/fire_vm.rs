use crate::drivers::vmm::appengine::prelude::*;
use crate::drivers::vmm::appengine::models::base_vm::BaseVm;

#[derive(Clone, Debug)]
pub(crate) struct FireVm {
    pub(crate) base: BaseVm,
    pub(crate) socket_path: String,
    pub(crate) kernel_image_path: String,
    pub(crate) rootfs_path: String,
}
