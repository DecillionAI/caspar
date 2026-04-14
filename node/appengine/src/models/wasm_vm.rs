#[derive(Clone, Debug)]
struct WasmVm {
    base: BaseVm,
    module_path: String,
    entry_point: String,
}
