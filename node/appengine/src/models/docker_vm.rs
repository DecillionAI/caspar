#[derive(Clone, Debug)]
struct DockerVm {
    base: BaseVm,
    container_id: String,
    image_ref: String,
    env: Vec<String>,
}
