#[derive(Clone, Debug)]
struct JavascriptVm {
    base: BaseVm,
    script_path: String,
    transpiled_masm_path: String,
}
