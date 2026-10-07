use std::sync::Arc;
use yang_drivers::transport::serial_discovery::SerialDeviceInfo;
use yang_worker::{
    discovery::{Discovery, InventoryPort},
    WorkerError,
};
struct Ports;
impl InventoryPort for Ports {
    fn serial(&self) -> Result<Vec<SerialDeviceInfo>, WorkerError> {
        Ok(vec![SerialDeviceInfo {
            resource: "COM12".into(),
            vid: Some(0x10c4),
            pid: Some(0xea60),
            serial: "candidate".into(),
            description: "USB UART".into(),
        }])
    }
    fn visa(&self) -> Result<Vec<String>, WorkerError> {
        Err(WorkerError::new(
            "DependencyUnavailable",
            "VISA not installed",
        ))
    }
}
#[test]
fn missing_visa_does_not_hide_serial_inventory() {
    let inventory = Discovery::new(Arc::new(Ports)).inventory();
    assert_eq!(inventory.serial.len(), 1);
    assert!(inventory.visa.is_empty());
    assert!(inventory.errors["visa"].contains("DependencyUnavailable"));
    let value = serde_json::to_value(inventory).unwrap();
    assert!(value["serial"][0].get("model").is_none());
    assert!(value["serial"][0].get("verified").is_none());
}
