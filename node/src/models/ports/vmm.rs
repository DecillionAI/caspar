use crate::models::worker::Trx;

/// The virtual machine manager driver interface.
pub trait IVmm: Send + Sync {
    fn assign(&self, machine_id: &str);
    fn run_vm(&self, machine_id: &str, store_id: &str, data: &str);
    fn terminate_vm(&self, machine_id: &str);
    fn build_vm_image(
        &self,
        machine_id: &str,
        entity_id: &str,
        build_path: &str,
        build_type: &str,
    );
    fn execute_chain_trxs_group(&self, trxs: Vec<Trx>);
    fn execute_chain_effects(&self, effects: &str);
    fn close_kvdb(&self);
    /// Returns `(result, gasUsed)`.
    fn vm_callback(&self, data_raw: &str) -> (String, i64);
}
