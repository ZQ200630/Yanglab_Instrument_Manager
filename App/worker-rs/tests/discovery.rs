use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use yang_drivers::{
    transport::serial_discovery::SerialDeviceInfo,
    usb_drivers::{
        self, DriverState, NewportInventory, SdkStatus, UsbDeviceRecord, UsbSerialInventory,
    },
};
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
        Err(WorkerError::new("DependencyUnavailable", "VISA not installed"))
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

#[test]
fn unconfigured_metadata_is_unavailable_and_keeps_partial_inventory() {
    let value = serde_json::to_value(Discovery::new(Arc::new(Ports)).inventory()).unwrap();
    assert_eq!(value["usb_serial"]["ch340"]["state"], "unavailable");
    assert_eq!(value["usb_serial"]["cp210x"]["state"], "unavailable");
    assert_eq!(value["newport"]["sdk"]["state"], "unavailable");
    assert!(value["errors"]["newport"].is_string());
    assert!(value["errors"]["usb_serial"].is_string());
    assert_eq!(value["serial"].as_array().unwrap().len(), 1);
    assert!(value["newport"].get("state").is_none());
}

struct MetadataPorts {
    calls: AtomicUsize,
    case: &'static str,
}
impl InventoryPort for MetadataPorts {
    fn serial(&self) -> Result<Vec<SerialDeviceInfo>, WorkerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.case == "serial" {
            return Err(WorkerError::new("DependencyUnavailable", "serial failed"));
        }
        Ok(vec![SerialDeviceInfo {
            resource: "COM2".into(),
            vid: None,
            pid: None,
            serial: "2110148249-10".into(),
            description: "left".into(),
        }; 2])
    }
    fn visa(&self) -> Result<Vec<String>, WorkerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(WorkerError::new("DependencyUnavailable", "VISA failed"))
    }
    fn newport(&self) -> Result<NewportInventory, WorkerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.case == "newport" {
            return Err(WorkerError::new("Native", "metadata denied"));
        }
        let sdk = if self.case == "ready" {
            SdkStatus {
                state: DriverState::Ready,
                path: Some("C:\\VendorRoot\\Newport\\Newport USB Driver\\Bin\\usbdll.dll".into()),
                bits: Some(64),
                message: "Compatible SDK file found; communication has not been tested.".into(),
            }
        } else {
            SdkStatus {
                state: DriverState::Missing,
                path: None,
                bits: None,
                message: "SDK absent".into(),
            }
        };
        Ok(usb_drivers::newport_from_records(&[], sdk).unwrap())
    }
    fn usb_serial(&self) -> Result<UsbSerialInventory, WorkerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.case == "usb_serial" {
            return Err(WorkerError::new("Native", "status denied"));
        }
        let records = if self.case == "empty" {
            vec![]
        } else {
            vec![UsbDeviceRecord {
                instance_id: "USB\\VID_1A86&PID_7523\\finite".into(),
                service: if self.case == "ready" { "CH341SER" } else { "" }.into(),
                description: "USB adapter".into(),
                problem_code: Some(if self.case == "ready" { 0 } else { 28 }),
            }]
        };
        Ok(usb_drivers::serial_usb_from_records(&records).unwrap())
    }
}

#[test]
fn construction_is_lazy_and_provider_failures_do_not_hide_independent_evidence() {
    for case in ["", "serial", "newport", "usb_serial"] {
        let port = Arc::new(MetadataPorts { calls: AtomicUsize::new(0), case });
        let discovery = Discovery::new(port.clone());
        assert_eq!(port.calls.load(Ordering::SeqCst), 0);
        let inventory = discovery.inventory();
        assert_eq!(port.calls.load(Ordering::SeqCst), 4);
        assert!(inventory.errors.contains_key("visa"));
        assert_eq!(inventory.errors.contains_key("fiber"), case != "serial");
        assert!(inventory.fiber["left"].is_null());
        assert!(inventory.fiber["right"].is_null());
        let newport_state = if case == "newport" {
            DriverState::Unavailable
        } else {
            DriverState::Missing
        };
        let ch_state = if case == "usb_serial" {
            DriverState::Unavailable
        } else {
            DriverState::Missing
        };
        let cp_state = if case == "usb_serial" {
            DriverState::Unavailable
        } else {
            DriverState::NotDetected
        };
        assert_eq!(inventory.newport.sdk.state, newport_state);
        assert_eq!(inventory.usb_serial.ch340.state, ch_state);
        assert_eq!(inventory.usb_serial.cp210x.state, cp_state);
        assert_eq!(inventory.errors.contains_key("newport"), case == "newport");
        assert_eq!(inventory.errors.contains_key("usb_serial"), case == "usb_serial");
        let value = serde_json::to_value(inventory).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 7);
        assert!(value["newport"]["devices"].as_array().unwrap().is_empty());
        assert!(value["newport"].get("state").is_none());
    }
}

#[test]
fn successful_empty_and_ready_metadata_preserve_legacy_install_evidence() {
    for (case, expected) in [("ready", "ready"), ("empty", "not_detected")] {
        let port = Arc::new(MetadataPorts { calls: AtomicUsize::new(0), case });
        let value = serde_json::to_value(Discovery::new(port).inventory()).unwrap();
        assert_eq!(value["usb_serial"]["ch340"]["state"], expected);
        assert_eq!(value["usb_serial"]["cp210x"]["state"], "not_detected");
        assert!(value["errors"].get("newport").is_none());
        assert!(value["errors"].get("usb_serial").is_none());
        assert!(value["newport"]["devices"].as_array().unwrap().is_empty());
        if case == "ready" {
            assert_eq!(value["newport"]["sdk"]["state"], "ready");
            assert_eq!(value["newport"]["sdk"]["bits"], 64);
            assert!(value["newport"]["sdk"]["path"].is_string());
        } else {
            assert_eq!(value["newport"]["sdk"]["state"], "missing");
            assert!(value["newport"]["sdk"].get("path").is_none());
        }
    }
}
