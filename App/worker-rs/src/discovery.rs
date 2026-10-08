use crate::WorkerError;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
use yang_drivers::transport::serial_discovery::SerialDeviceInfo;
use yang_drivers::usb_drivers::{NewportInventory, UsbSerialInventory};
pub trait InventoryPort: Send + Sync {
    fn serial(&self) -> Result<Vec<SerialDeviceInfo>, WorkerError>;
    fn visa(&self) -> Result<Vec<String>, WorkerError>;
    fn newport(&self) -> Result<NewportInventory, WorkerError> {
        Err(WorkerError::new(
            "DependencyUnavailable",
            "Newport metadata provider not configured",
        ))
    }
    fn usb_serial(&self) -> Result<UsbSerialInventory, WorkerError> {
        Err(WorkerError::new(
            "DependencyUnavailable",
            "USB serial metadata provider not configured",
        ))
    }
}
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct Inventory {
    pub serial: Vec<SerialDeviceInfo>,
    pub visa: Vec<String>,
    pub errors: BTreeMap<String, String>,
    pub suggestions: BTreeMap<String, Vec<String>>,
    pub fiber: serde_json::Value,
    pub newport: NewportInventory,
    pub usb_serial: UsbSerialInventory,
}
pub struct Discovery(Arc<dyn InventoryPort>);
impl Discovery {
    pub fn new(port: Arc<dyn InventoryPort>) -> Self {
        Self(port)
    }
    pub fn inventory(&self) -> Inventory {
        let mut result = Inventory {
            suggestions: BTreeMap::from([("gain".into(), vec![]), ("voltage".into(), vec![])]),
            fiber: serde_json::json!({"left":null,"right":null,"unknown":[]}),
            ..Inventory::default()
        };
        match self.0.serial() {
            Ok(serial) if serial.len() <= 4096 => result.serial = serial,
            Ok(_) => {
                result
                    .errors
                    .insert("serial".into(), "Inventory exceeds 4096 records".into());
            }
            Err(error) => {
                result.errors.insert("serial".into(), error.to_string());
            }
        }
        match self.0.visa() {
            Ok(visa) if visa.len() <= 4096 => result.visa = visa,
            Ok(_) => {
                result
                    .errors
                    .insert("visa".into(), "Inventory exceeds 4096 records".into());
            }
            Err(error) => {
                result.errors.insert("visa".into(), error.to_string());
            }
        }
        match self.0.newport() {
            Ok(newport) => result.newport = newport,
            Err(error) => {
                result.errors.insert("newport".into(), error.to_string());
            }
        }
        match self.0.usb_serial() {
            Ok(usb_serial) => result.usb_serial = usb_serial,
            Err(error) => {
                result.errors.insert("usb_serial".into(), error.to_string());
            }
        }
        for record in &result.serial {
            let side = match record.serial.as_str() {
                "2110148249-10" => Some("left"),
                "160721175410" => Some("right"),
                _ => None,
            };
            if let Some(side) = side {
                if !result.fiber[side].is_null() {
                    result.errors.insert(
                        "fiber".into(),
                        "Duplicate registered USB serial; side is ambiguous".into(),
                    );
                } else {
                    result.fiber[side] = serde_json::to_value(record).unwrap();
                }
            } else if record.vid == Some(0x1a86) && record.pid == Some(0x7523) {
                result
                    .suggestions
                    .get_mut("voltage")
                    .unwrap()
                    .push(record.resource.clone());
            } else if record.vid == Some(0x10c4) && record.pid == Some(0xea60) {
                result
                    .suggestions
                    .get_mut("gain")
                    .unwrap()
                    .push(record.resource.clone());
            } else if record.vid == Some(0x0403) {
                result.fiber["unknown"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::to_value(record).unwrap());
            }
        }
        if result.errors.contains_key("fiber") {
            result.fiber["left"] = serde_json::Value::Null;
            result.fiber["right"] = serde_json::Value::Null;
        }
        result
    }
}

/// Invoked only by an authorized enumeration request, never during construction.
pub struct SystemInventory;
impl InventoryPort for SystemInventory {
    fn newport(&self) -> Result<NewportInventory, WorkerError> {
        Ok(yang_drivers::usb_drivers::system_newport()?)
    }
    fn usb_serial(&self) -> Result<UsbSerialInventory, WorkerError> {
        Ok(yang_drivers::usb_drivers::system_usb_serial()?)
    }
    fn serial(&self) -> Result<Vec<SerialDeviceInfo>, WorkerError> {
        Ok(yang_drivers::transport::serial_discovery::enumerate_serial()?)
    }
    fn visa(&self) -> Result<Vec<String>, WorkerError> {
        let manager = yang_drivers::transport::visa::VisaManager::load_system(Default::default())?;
        Ok(manager.enumerate()?)
    }
}
