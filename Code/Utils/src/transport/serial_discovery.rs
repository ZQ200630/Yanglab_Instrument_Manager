use crate::{DriverError, DriverResult};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialDeviceInfo {
    pub resource: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub serial: String,
    pub description: String,
}
#[derive(Clone, Debug)]
pub struct DeviceRecord {
    pub port: String,
    pub instance_id: String,
    pub parents: Vec<String>,
    pub description: String,
}
fn hex_id(identity: &str, tag: &str) -> Option<u16> {
    let upper = identity.to_ascii_uppercase();
    let start = upper.find(tag)? + tag.len();
    u16::from_str_radix(upper.get(start..start + 4)?, 16).ok()
}
fn identity_serial(identity: &str) -> Option<String> {
    let upper = identity.to_ascii_uppercase();
    if upper.starts_with("FTDIBUS\\") {
        return identity
            .split('+')
            .nth(2)
            .and_then(|s| s.split('\\').next())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
    }
    if !upper.starts_with("USB\\") || upper.contains("&MI_") {
        return None;
    }
    let serial = identity.split('\\').nth(2)?;
    // Windows topology IDs contain '&'; never present them as a USB serial.
    if serial.is_empty() || serial.contains('&') {
        None
    } else {
        Some(serial.into())
    }
}
pub fn records_to_devices(records: Vec<DeviceRecord>) -> DriverResult<Vec<SerialDeviceInfo>> {
    if records.len() > 4096 {
        return Err(DriverError::Protocol(
            "too many serial inventory records".into(),
        ));
    }
    let mut devices = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for record in records {
        let Ok(resource) = super::serial::canonical_com(&record.port) else {
            continue;
        };
        if !seen.insert(resource.clone()) {
            return Err(DriverError::Protocol(
                "duplicate COM inventory entry".into(),
            ));
        }
        let vid = hex_id(&record.instance_id, "VID_");
        let pid = hex_id(&record.instance_id, "PID_");
        let serial = identity_serial(&record.instance_id)
            .or_else(|| {
                record.parents.iter().take(8).find_map(|parent| {
                    if hex_id(parent, "VID_") == vid && hex_id(parent, "PID_") == pid {
                        identity_serial(parent)
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default();
        devices.push(SerialDeviceInfo {
            resource: resource.as_str().strip_prefix("serial://").unwrap().into(),
            vid,
            pid,
            serial,
            description: record.description,
        });
    }
    Ok(devices)
}
pub fn enumerate_serial() -> DriverResult<Vec<SerialDeviceInfo>> {
    super::serial::enumerate_with(&super::serial_abi::NativeSerial)
}
