#[derive(Clone, Debug)]
struct FireVm {
    base: BaseVm,
    socket_path: String,
    kernel_image_path: String,
    rootfs_path: String,
}
