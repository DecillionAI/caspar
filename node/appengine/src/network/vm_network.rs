struct VmNetworkService;

impl VmNetworkService {
    fn gateway_network_name() -> &'static str {
        "kasper"
    }

    fn firecracker_socket(machine_id: &str, vm_id: &str) -> String {
        format!("/opt/firecracker/vms/fc.{}.{}.sock", machine_id, vm_id)
    }

    fn vm_http_gateway_url(machine_id: &str, vm_id: &str, port: u16) -> String {
        format!(
            "http://127.0.0.1:8080/vm/{}/{}/{}",
            machine_id.trim(),
            vm_id.trim(),
            port
        )
    }
}
