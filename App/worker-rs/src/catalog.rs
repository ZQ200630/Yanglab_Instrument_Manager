use crate::WorkerError as HostError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const CATEGORIES: [&str; 8] = [
    "OSA",
    "ESA",
    "Oscilloscope",
    "Function Generator",
    "Power Meter",
    "Piezo Controller",
    "Laser",
    "Custom",
];
pub const DOCUMENT: &[u8] = include_bytes!("../../catalog/devices.json");

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub choices: Option<Vec<Value>>,
    #[serde(default)]
    pub minimum: Option<f64>,
    #[serde(default)]
    pub maximum: Option<f64>,
    #[serde(default)]
    pub interfaces: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub interfaces: Vec<String>,
    pub access: String,
    pub probe_mode: String,
    pub probe_version: u64,
    pub automatic_probe: bool,
    pub communication_ttl_s: u64,
    pub dependencies: Vec<String>,
    pub transport_hint: Option<Value>,
    pub open_effects: Vec<String>,
    pub fields: BTreeMap<String, Field>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub manufacturer: String,
    pub name: String,
    pub category: String,
    pub driver_id: String,
    pub driver_kind: String,
    pub driver_version: u64,
    pub dependencies: Vec<String>,
    pub connect_effects: Vec<String>,
    pub close_effects: Vec<String>,
    pub operations: Vec<String>,
    pub profiles: Vec<Profile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupModel {
    pub kind: String,
    pub model_id: String,
    pub driver_id: String,
    pub driver_kind: String,
    pub name: String,
    pub members: Vec<String>,
    pub operations: Vec<String>,
    pub close_effects: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub version: u64,
    pub categories: Vec<String>,
    pub models: Vec<Model>,
    pub setups: Vec<SetupModel>,
}

fn invalid(message: impl Into<String>) -> HostError {
    HostError::new("CatalogInvalid", message)
}

fn strings_valid(items: &[String]) -> bool {
    items.iter().all(|item| !item.is_empty())
        && items.iter().collect::<BTreeSet<_>>().len() == items.len()
}

fn digits(value: &str, empty: bool) -> bool {
    (empty || !value.is_empty()) && value.bytes().all(|char| char.is_ascii_digit())
}

fn reviewed_resource(value: &str, interfaces: &[String]) -> bool {
    let parts: Vec<_> = value.split("::").collect();
    let gpib = matches!(parts.len(), 3 | 4)
        && parts[0]
            .strip_prefix("GPIB")
            .is_some_and(|board| digits(board, true))
        && parts.last() == Some(&"INSTR")
        && parts[1..parts.len() - 1]
            .iter()
            .all(|address| (1..=2).contains(&address.len()) && digits(address, false));
    let usb = matches!(parts.len(), 5 | 6)
        && parts[0]
            .strip_prefix("USB")
            .is_some_and(|board| digits(board, true))
        && parts.last() == Some(&"INSTR")
        && parts[1..3]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|char| char.is_ascii_alphanumeric()))
        && !parts[3].is_empty()
        && parts[3]
            .bytes()
            .all(|char| char.is_ascii_alphanumeric() || b"_.-".contains(&char))
        && (parts.len() == 5 || digits(parts[4], false));
    (gpib && interfaces.iter().any(|face| face == "GPIB"))
        || (usb && interfaces.iter().any(|face| face == "USB"))
}

impl Profile {
    pub fn validate(&self, params: &Value) -> Result<Value, HostError> {
        let params = params
            .as_object()
            .ok_or_else(|| invalid("Connection must be an object"))?;
        if params.keys().any(|key| !self.fields.contains_key(key)) {
            return Err(invalid("Unknown connection parameter"));
        }
        let mut result = Map::new();
        for (name, rule) in &self.fields {
            let Some(value) = params.get(name).or(rule.default.as_ref()) else {
                if rule.required {
                    return Err(invalid(format!("Missing connection parameter: {name}")));
                }
                continue;
            };
            let mut value = value.clone();
            match rule.kind.as_str() {
                "text" | "serial" | "resource" => {
                    let raw = value.as_str().ok_or_else(|| invalid("Expected text"))?;
                    if raw.is_empty()
                        || raw.chars().count() > 256
                        || raw.chars().any(|char| char < ' ' || char == '\u{7f}')
                    {
                        return Err(invalid("Invalid connection text"));
                    }
                    if rule.kind == "serial" {
                        let port = raw.to_ascii_uppercase();
                        let number = port
                            .strip_prefix("COM")
                            .ok_or_else(|| invalid("Expected a COM port"))?;
                        if !(1..=4).contains(&number.len())
                            || number.starts_with('0')
                            || !digits(number, false)
                        {
                            return Err(invalid("Expected a Windows COM port"));
                        }
                        value = Value::String(port);
                    } else if rule.kind == "resource" && !reviewed_resource(raw, &rule.interfaces) {
                        return Err(invalid("VISA interface is not reviewed for this profile"));
                    }
                }
                "integer" if value.is_i64() || value.is_u64() => {}
                "number" if value.as_f64().is_some_and(f64::is_finite) => {}
                _ => return Err(invalid("Invalid connection field type")),
            }
            if rule
                .choices
                .as_ref()
                .is_some_and(|choices| !choices.contains(&value))
            {
                return Err(invalid("Value outside reviewed choices"));
            }
            if rule
                .minimum
                .is_some_and(|minimum| value.as_f64().map_or(true, |v| v < minimum))
                || rule
                    .maximum
                    .is_some_and(|maximum| value.as_f64().map_or(true, |v| v > maximum))
            {
                return Err(invalid("Connection value outside allowed limits"));
            }
            result.insert(name.clone(), value);
        }
        Ok(Value::Object(result))
    }
}

impl Catalog {
    pub fn load(bytes: &[u8]) -> Result<Self, HostError> {
        let catalog: Self = serde_json::from_slice(bytes)
            .map_err(|error| invalid(format!("Cannot decode trusted catalog: {error}")))?;
        if catalog.version != 1 || catalog.categories != CATEGORIES {
            return Err(invalid("Unsupported catalog version or categories"));
        }
        let mut ids = BTreeSet::new();
        for model in &catalog.models {
            let (kind, category, profile_id, access, face) = match model.id.as_str() {
                "aq6370" => ("osa", "OSA", "gpib-visa", "visa", "GPIB"),
                "voltage" => ("voltage", "Custom", "ch340-serial", "serial", "Serial"),
                "gain" => ("gain", "Custom", "cp210x-serial", "serial", "Serial"),
                "pm400" => ("pm400", "Power Meter", "usb-visa", "visa", "USB"),
                "mdt693b" => ("mdt", "Piezo Controller", "serial", "serial", "Serial"),
                "tlb6700" => ("laser", "Laser", "newport-usb", "newport", "USB"),
                _ => return Err(invalid("Unknown trusted model")),
            };
            if !ids.insert(&model.id)
                || model.driver_kind != kind
                || model.driver_id != model.id
                || model.driver_version == 0
                || model.category != category
                || model.manufacturer.is_empty()
                || model.name.is_empty()
                || !strings_valid(&model.dependencies)
                || !strings_valid(&model.connect_effects)
                || !strings_valid(&model.close_effects)
                || !strings_valid(&model.operations)
            {
                return Err(invalid("Invalid model or trusted driver binding"));
            }
            let mut profiles = BTreeSet::new();
            for profile in &model.profiles {
                if !profiles.insert(&profile.id)
                    || profile.id != profile_id
                    || profile.access != access
                    || profile.interfaces != [face]
                    || profile.probe_version == 0
                    || profile.communication_ttl_s == 0
                    || !matches!(profile.access.as_str(), "visa" | "serial" | "newport")
                    || !matches!(
                        profile.probe_mode.as_str(),
                        "readonly_pending" | "readonly" | "supervised"
                    )
                    || (profile.automatic_probe && profile.probe_mode != "readonly")
                    || !strings_valid(&profile.interfaces)
                    || !strings_valid(&profile.dependencies)
                    || !strings_valid(&profile.open_effects)
                {
                    return Err(invalid("Invalid connection profile"));
                }
                for rule in profile.fields.values() {
                    if !matches!(
                        rule.kind.as_str(),
                        "text" | "serial" | "resource" | "integer" | "number"
                    ) || rule.choices.as_ref().is_some_and(Vec::is_empty)
                        || rule.minimum.is_some_and(|v| !v.is_finite())
                        || rule.maximum.is_some_and(|v| !v.is_finite())
                    {
                        return Err(invalid("Invalid field rule"));
                    }
                }
                let expected: &[(&str, &str)] = if access == "newport" {
                    &[
                        ("device_key", "text"),
                        ("operating_min_nm", "number"),
                        ("operating_max_nm", "number"),
                        ("scan_speed_limit_nm_s", "number"),
                    ]
                } else if access == "visa" {
                    &[
                        ("resource", "resource"),
                        ("backend", "text"),
                        ("timeout_s", "number"),
                    ]
                } else {
                    &[
                        ("port", "serial"),
                        ("baudrate", "integer"),
                        ("io_timeout_s", "number"),
                    ]
                };
                if profile.fields.len() != expected.len()
                    || expected.iter().any(|(name, kind)| {
                        profile
                            .fields
                            .get(*name)
                            .map_or(true, |rule| rule.kind != *kind)
                    })
                {
                    return Err(invalid("Unreviewed connection fields"));
                }
                if access == "serial" {
                    let baud = &profile.fields["baudrate"];
                    if baud.default != Some(Value::from(115200))
                        || baud.choices != Some(vec![Value::from(115200)])
                    {
                        return Err(invalid("Serial baudrate is fixed"));
                    }
                } else if access == "visa" {
                    let backend = &profile.fields["backend"];
                    if backend.default != Some(Value::from("system"))
                        || backend.choices != Some(vec![Value::from("system")])
                        || profile.fields["resource"].interfaces != [face]
                    {
                        return Err(invalid("VISA backend/interface is fixed"));
                    }
                }
                let sample = if access == "newport" {
                    serde_json::json!({"device_key":"6700 SN1012"})
                } else if access == "serial" {
                    serde_json::json!({"port":"COM1"})
                } else if face == "GPIB" {
                    serde_json::json!({"resource":"GPIB0::4::INSTR"})
                } else {
                    serde_json::json!({"resource":"USB0::0x1313::0x807B::TEST::INSTR"})
                };
                profile.validate(&sample)?;
            }
        }
        for setup in &catalog.setups {
            if setup.kind != "fiber"
                || setup.model_id != "fiber-coupling"
                || setup.driver_id != "fiber"
                || setup.driver_kind != "fiber"
                || setup.members != ["2110148249-10", "160721175410"]
            {
                return Err(invalid("Unknown setup binding"));
            }
        }
        Ok(catalog)
    }

    pub fn model(&self, id: &str) -> Result<&Model, HostError> {
        self.models
            .iter()
            .find(|model| model.id == id)
            .ok_or_else(|| HostError::new("DriverRequired", "Driver required for this model"))
    }

    pub fn profile(&self, model: &str, profile: &str) -> Result<&Profile, HostError> {
        self.model(model)?
            .profiles
            .iter()
            .find(|item| item.id == profile)
            .ok_or_else(|| invalid("Unreviewed connection profile"))
    }

    pub fn validate_connection(
        &self,
        model: &str,
        profile: &str,
        params: &Value,
    ) -> Result<Value, HostError> {
        self.profile(model, profile)?.validate(params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DOCUMENT: &[u8] = include_bytes!("../../catalog/devices.json");

    #[test]
    fn reviewed_profile_applies_defaults_without_inventing_network_support() {
        let catalog = Catalog::load(DOCUMENT).expect("reviewed catalog must load");
        assert_eq!(
            catalog
                .validate_connection("gain", "cp210x-serial", &json!({"port":"com12"}))
                .unwrap(),
            json!({"port":"COM12","baudrate":115200,"io_timeout_s":1})
        );
        assert!(catalog
            .validate_connection(
                "aq6370",
                "tcpip-visa",
                &json!({"resource":"TCPIP0::10.0.0.1::INSTR"})
            )
            .is_err());
    }

    #[test]
    fn unreviewed_connection_values_never_pass_validation() {
        let catalog = Catalog::load(DOCUMENT).unwrap();
        for value in [
            json!({"port":"COM0"}),
            json!({"port":"COM3","baudrate":9600}),
            json!({"port":"COM3","baudrate":true}),
            json!({"port":"COM3","raw_command":"STCA999999"}),
            json!({"port":"COM3","io_timeout_s":0}),
        ] {
            assert!(catalog
                .validate_connection("gain", "cp210x-serial", &value)
                .is_err());
        }
        for value in [
            json!({"resource":"TCPIP0::10.0.0.1::INSTR"}),
            json!({"resource":"GPIB0::4::INSTR","backend":"@py"}),
            json!({"resource":"GPIB0::4::INSTR\n*RST"}),
            json!({"resource":"GPIB0::4::INSTR","timeout_s":true}),
        ] {
            assert!(catalog
                .validate_connection("aq6370", "gpib-visa", &value)
                .is_err());
        }
    }

    #[test]
    fn unknown_driver_and_duplicate_profiles_fail_closed() {
        assert!(
            Catalog::load(DOCUMENT).is_ok(),
            "Reviewed catalog must be accepted"
        );
        let original: Value = serde_json::from_slice(DOCUMENT).unwrap();
        let mut driver = original.clone();
        driver["models"][0]["driver_kind"] = json!("user.module");
        assert!(Catalog::load(&serde_json::to_vec(&driver).unwrap()).is_err());
        let mut duplicate = original;
        let first = duplicate["models"][0]["profiles"][0].clone();
        duplicate["models"][0]["profiles"]
            .as_array_mut()
            .unwrap()
            .push(first);
        assert!(Catalog::load(&serde_json::to_vec(&duplicate).unwrap()).is_err());
    }

    #[test]
    fn catalog_cannot_introduce_unreviewed_profiles_or_invalid_defaults() {
        let original: Value = serde_json::from_slice(DOCUMENT).unwrap();
        for (pointer, replacement) in [
            ("/models/0/profiles/0/id", json!("tcpip-visa")),
            ("/models/0/profiles/0/interfaces", json!(["TCPIP"])),
            ("/models/2/profiles/0/fields/baudrate/default", json!(9600)),
        ] {
            let mut modified = original.clone();
            *modified.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                Catalog::load(&serde_json::to_vec(&modified).unwrap()).is_err(),
                "{pointer}"
            );
        }
    }
}
pub fn admit(
    config: &yang_protocol::DomainConfig,
) -> Result<yang_protocol::DomainConfig, HostError> {
    yang_protocol::validate_config(config)?;
    let mut approved = config.clone();
    if config.domain.kind == "device" {
        let catalog = Catalog::load(DOCUMENT)?;
        let model = catalog.model(&config.model_id)?;
        if model.driver_kind != config.driver_kind {
            return Err(invalid("Model/driver mismatch"));
        }
        approved.params = catalog.validate_connection(
            &config.model_id,
            config.profile_id.as_deref().unwrap(),
            &config.params,
        )?;
        if config.driver_kind == "laser" {
            let key = approved.params["device_key"].as_str()
                .filter(|k| yang_lab_tlb::valid_key(k))
                .ok_or_else(|| invalid("Exact Newport controller key required"))?;
            if config.expected_identity.get("serial")
                .is_some_and(|v| v.as_str() != Some(&key[7..])) {
                return Err(invalid("Controller serial differs from its key"));
            }
            laser_limits(
                &approved.params,
                config.expected_identity["head_model"].as_str(),
            )?;
        }
    } else {
        approved.members = config.members.iter().map(admit).collect::<Result<_, _>>()?;
        for (name, min, max) in [
            ("voltage_limit_v", 0.1, 75.0),
            ("toward_chip_limit_um", 0.000001, 0.2),
            ("other_limit_um", 0.000001, 1.0),
            ("serial_timeout_s", 0.05, 5.0),
        ] {
            if let Some(value) = config.params.get(name) {
                if !value
                    .as_f64()
                    .is_some_and(|n| n.is_finite() && n >= min && n <= max)
                {
                    return Err(invalid("Setup parameter exceeds reviewed bounds"));
                }
            }
        }
    }
    physical_keys(&approved)?;
    Ok(approved)
}

#[derive(Clone, Default)]
pub struct Claims(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, ClaimEntry>>>);
struct ClaimEntry {
    domain: yang_protocol::DomainRef,
    keys: BTreeSet<String>,
}
#[derive(Clone)]
pub struct ClaimToken {
    token: String,
    book: Claims,
}
impl Claims {
    pub fn reserve(&self, config: &yang_protocol::DomainConfig) -> Result<ClaimToken, HostError> {
        let approved = admit(config)?;
        let keys = physical_keys(&approved)?;
        let mut entries = self
            .0
            .lock()
            .map_err(|_| invalid("Physical claim book poisoned"))?;
        if entries.len() >= 64
            || entries
                .values()
                .any(|entry| entry.domain == config.domain || !entry.keys.is_disjoint(&keys))
        {
            return Err(HostError::new(
                "ResourceBusy",
                "Physical resource/member is already owned",
            ));
        }
        let token = crate::new_id()?;
        entries.insert(
            token.clone(),
            ClaimEntry {
                domain: config.domain.clone(),
                keys,
            },
        );
        Ok(ClaimToken {
            token,
            book: self.clone(),
        })
    }
    pub fn confirm_release(
        &self,
        token: &ClaimToken,
        report: &yang_drivers::lifecycle::CleanupReport,
    ) -> Result<(), HostError> {
        if !std::sync::Arc::ptr_eq(&self.0, &token.book.0) || !report.resources_released() {
            return Err(HostError::new(
                "ReleaseUnconfirmed",
                "Wrong owner or unreleased physical responsibility",
            ));
        }
        let mut entries = self
            .0
            .lock()
            .map_err(|_| invalid("Physical claim book poisoned"))?;
        if entries.remove(&token.token).is_none() {
            return Err(invalid("Stale physical reservation"));
        }
        Ok(())
    }
}

/// Pure metadata normalization. Arbitrary VISA aliases are rejected at admission;
/// the native manager performs its authoritative parse before any session opens.
pub fn physical_keys(config: &yang_protocol::DomainConfig) -> Result<BTreeSet<String>, HostError> {
    if config.domain.kind == "setup" {
        let mut keys = BTreeSet::new();
        for member in &config.members {
            for key in physical_keys(member)? {
                if !keys.insert(key) {
                    return Err(invalid("Repeated physical setup member"));
                }
            }
        }
        return Ok(keys);
    }
    let mut keys = BTreeSet::new();
    if let Some(port) = config.params["port"].as_str() {
        keys.insert(
            yang_drivers::transport::serial::canonical_com(port)?
                .as_str()
                .to_owned(),
        );
    } else if config.driver_kind == "laser" {
        let key = config.params["device_key"].as_str()
            .filter(|k| yang_lab_tlb::valid_key(k))
            .ok_or_else(|| invalid("Exact Newport controller key required"))?;
        keys.insert(format!("newport://{key}"));
        if let Some(serial) = config.expected_identity["head_serial"].as_str() {
            if serial.is_empty() || serial.len() > 64
                || !serial.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
                return Err(invalid("Invalid laser-head serial"));
            }
            keys.insert(format!("instrument://tlb-head:{serial}"));
        }
    } else if let Some(resource) = config.params["resource"].as_str() {
        let parts: Vec<_> = resource.split("::").collect();
        let number = |text: &str| -> Result<u32, HostError> {
            let parsed =
                if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                    u32::from_str_radix(hex, 16)
                } else {
                    text.parse()
                };
            parsed.map_err(|_| invalid("Unresolved VISA numeric address"))
        };
        let canonical = if let Some(board) = parts[0].strip_prefix("GPIB") {
            let mut fields = vec![format!(
                "GPIB{}",
                number(if board.is_empty() { "0" } else { board })?
            )];
            for part in &parts[1..parts.len() - 1] {
                fields.push(number(part)?.to_string());
            }
            fields.push("INSTR".into());
            fields.join("::")
        } else if let Some(board) = parts[0].strip_prefix("USB") {
            if !(5..=6).contains(&parts.len()) {
                return Err(invalid("Unresolved USB descriptor"));
            }
            format!(
                "USB{}::{:#x}::{:#x}::{}::{}::INSTR",
                number(if board.is_empty() { "0" } else { board })?,
                number(parts[1])?,
                number(parts[2])?,
                parts[3],
                if parts.len() == 6 {
                    number(parts[4])?
                } else {
                    0
                }
            )
        } else {
            return Err(invalid("Unreviewed VISA descriptor"));
        };
        keys.insert(format!("visa://{canonical}"));
    } else {
        return Err(invalid("Missing physical address"));
    }
    for field in ["serial", "transport_serial"] {
        if let Some(value) = config.expected_identity.get(field) {
            let serial = value
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 256)
                .ok_or_else(|| invalid("Invalid instrument/transport serial"))?;
            keys.insert(if field == "serial" {
                format!("instrument://{}:{serial}", config.model_id)
            } else {
                format!("transport://{serial}")
            });
        }
    }
    Ok(keys)
}

/// Collective operating limits; the live Bus checks the newly identified head again.
pub(crate) fn laser_limits(
    params: &Value,
    head: Option<&str>,
) -> Result<Option<yang_lab_tlb::ControlLimits>, HostError> {
    let keys = ["operating_min_nm", "operating_max_nm", "scan_speed_limit_nm_s"];
    if !keys.iter().any(|k| params.get(*k).is_some()) {
        return Ok(None);
    }
    let values = keys.map(|k| params[k].as_f64().filter(|v| v.is_finite()));
    let (Some(min_nm), Some(max_nm), Some(max_speed_nm_s)) = (values[0], values[1], values[2]) else {
        return Err(invalid("All operating limits must be finite and configured together"));
    };
    if min_nm >= max_nm || max_speed_nm_s < 0.01 {
        return Err(invalid("Invalid operating limits"));
    }
    if let Some(head) = head {
        let (a, b, speed) = yang_lab_tlb::model_spec(head)
            .ok_or_else(|| invalid("Unknown laser-head limits"))?;
        if min_nm < a || max_nm > b || max_speed_nm_s > speed {
            return Err(invalid("Operating limits may only narrow hardware limits"));
        }
    }
    Ok(Some(yang_lab_tlb::ControlLimits {
        min_nm, max_nm, max_speed_nm_s,
    }))
}
