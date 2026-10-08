//! Durable configuration only. The Host serializes this single writer.
use super::catalog::{Catalog, DOCUMENT};
use super::contracts::*;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 1_048_576;

fn failure(code: &str, message: impl Into<String>) -> HostError {
    HostError::new(code, message)
}
fn io_error(error: std::io::Error) -> HostError {
    failure("ConfigStorage", error.to_string())
}
fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= limit
        && !value.chars().any(|char| char < ' ' || char == '\u{7f}')
}

pub fn new_id() -> Result<String, HostError> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::Cryptography::{
            BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        };
        let mut bytes = [0_u8; 16];
        // SAFETY: a fixed valid mutable buffer; system RNG needs no algorithm handle.
        let result = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                bytes.as_mut_ptr(),
                16,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if result < 0 {
            return Err(failure(
                "RandomUnavailable",
                "Windows secure random generation failed",
            ));
        }
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }
    #[cfg(not(windows))]
    {
        Err(failure("UnsupportedPlatform", "This Host requires Windows"))
    }
}

/// Internal, non-serializable evidence capability. Only the responsibility layer
/// can mint one after independently checking cleanup and current configuration.
#[derive(Debug)]
pub struct ReleasePermit {
    device_id: String,
    config_rev: u64,
    attempt_id: String,
}
impl ReleasePermit {
    pub(crate) fn confirmed(device_id: String, config_rev: u64, attempt_id: String) -> Self {
        Self {
            device_id,
            config_rev,
            attempt_id,
        }
    }
}

pub enum RegistryChange {
    SaveDraft(DraftRecord),
    CancelDraft {
        device_id: String,
        config_rev: u64,
        permit: ReleasePermit,
    },
    SaveDevice(DeviceRecord),
    EditName {
        device_id: String,
        config_rev: u64,
        name: String,
    },
    LaserLimits {device_id:String,config_rev:u64,limits:Value},
    RetireDevice {
        device_id: String,
        config_rev: u64,
        permit: ReleasePermit,
    },
    SaveSetup(SetupRecord),
    RetireSetup {
        setup_id: String,
        config_rev: u64,
        permit: ReleasePermit,
    },
    SaveSettings(HostSettings),
    SaveCheckPolicy {
        device_id: String,
        config_rev: u64,
        policy: CheckPolicy,
    },
}

pub struct Migration {
    pub backup_bytes: Vec<u8>,
    pub devices: Vec<DeviceRecord>,
    pub drafts: Vec<DraftRecord>,
    pub settings: HostSettings,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Legacy {
    #[serde(default = "legacy_version")]
    version: u64,
    #[serde(default, rename = "python_path")]
    _obsolete_interpreter: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    bindings: BTreeMap<String, Option<String>>,
}
fn legacy_version() -> u64 {
    1
}

pub fn migrate_v1(bytes: &[u8]) -> Result<Migration, HostError> {
    let legacy: Legacy = serde_json::from_slice(bytes)
        .map_err(|error| failure("ConfigInvalid", error.to_string()))?;
    if legacy.version != 1 {
        return Err(failure("ConfigInvalid", "Not a version-1 configuration"));
    }
    let settings = HostSettings::default();
    let mut drafts = Vec::new();
    for (role, address) in legacy.bindings {
        let (model, profile, field) = match role.as_str() {
            "osa" => ("aq6370", "gpib-visa", "resource"),
            "pm400" => ("pm400", "usb-visa", "resource"),
            "voltage" => ("voltage", "ch340-serial", "port"),
            "gain" => ("gain", "cp210x-serial", "port"),
            _ => return Err(failure("ConfigInvalid", "Unknown legacy role")),
        };
        if let Some(address) = address {
            if !text(&address, 256) {
                return Err(failure("ConfigInvalid", "Invalid legacy address"));
            }
            let mut params = serde_json::Map::new();
            params.insert(field.into(), Value::String(address));
            drafts.push(DraftRecord {
                device_id: new_id()?,
                name: format!("Imported {role}"),
                model_id: Some(model.into()),
                profile_id: Some(profile.into()),
                params: Value::Object(params),
                config_digest: String::new(),
                revision: 1,
                mode: "unverified".into(),
                status: "Imported draft".into(),
            });
        }
    }
    Ok(Migration {
        backup_bytes: bytes.to_vec(),
        devices: Vec::new(),
        drafts,
        settings,
    })
}

pub(crate) trait AtomicStore: Send + Sync {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), HostError>;
    fn backup(&self, path: &Path, bytes: &[u8]) -> Result<BackupGuard, HostError>;
}
pub(crate) struct BackupGuard {
    _file: fs::File,
    _parents: Vec<fs::File>,
}
struct FileStore;
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".v1.bak");
    PathBuf::from(name)
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), HostError> {
    FileStore.write(path, bytes)
}
pub(crate) fn backup_original(path: &Path, bytes: &[u8]) -> Result<BackupGuard, HostError> {
    FileStore.backup(path, bytes)
}
// Pin existing ancestors before creating missing directories. Reparse paths
// are never followed, including when the leaf configuration does not exist.
fn config_parents(path: &Path) -> Result<Vec<fs::File>, HostError> {
    let parent = path
        .parent()
        .ok_or_else(|| failure("ConfigStorage", "Missing parent"))?;
    let mut missing = Vec::new();
    let mut existing = parent;
    while !existing.exists() {
        missing.push(existing);
        existing = existing
            .parent()
            .ok_or_else(|| failure("ConfigStorage", "Missing root"))?;
    }
    let mut guards = super::archive::pin_directories(existing)?;
    for directory in missing.into_iter().rev() {
        match fs::create_dir(directory) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(io_error(e)),
        }
        guards.push(super::archive::open_guarded(directory, true)?);
    }
    Ok(guards)
}
pub(crate) fn read_config(path: &Path, maximum: u64) -> Result<Vec<u8>, HostError> {
    let _parents = config_parents(path)?;
    fs::symlink_metadata(path).map_err(io_error)?;
    let file = super::archive::open_guarded(path, false)?;
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > maximum {
        return Err(failure(
            "ConfigCapacity",
            "Configuration exceeds bounded storage",
        ));
    }
    Ok(bytes)
}

impl AtomicStore for FileStore {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), HostError> {
        let parent = path
            .parent()
            .ok_or_else(|| failure("ConfigStorage", "Configuration needs a parent directory"))?;
        let _parents = config_parents(path).map_err(|e| failure("ConfigStorage", e.message))?;
        if fs::symlink_metadata(path).is_ok() {
            // Do not hold this handle through our own atomic replacement.
            drop(
                super::archive::open_guarded(path, false)
                    .map_err(|e| failure("ConfigStorage", e.message))?,
            );
        }
        let temporary = parent.join(format!(".registry-{}.tmp", new_id()?));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(io_error)?;
            file.write_all(bytes).map_err(io_error)?;
            file.flush().map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
            drop(file);
            #[cfg(windows)]
            {
                super::archive::rename_pinned(&temporary, path, true)
                    .map_err(|e| failure("ConfigStorage", e.message))
            }
            #[cfg(not(windows))]
            {
                Err(failure("UnsupportedPlatform", "This Host requires Windows"))
            }
        })();
        if temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
    fn backup(&self, path: &Path, bytes: &[u8]) -> Result<BackupGuard, HostError> {
        let parents = config_parents(path)?;
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                let result = file
                    .write_all(bytes)
                    .and_then(|_| file.flush())
                    .and_then(|_| file.sync_all())
                    .map_err(io_error);
                drop(file);
                if result.is_err() {
                    let _ = fs::remove_file(path);
                }
                result?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Verified below through the pinned no-follow handle.
            }
            Err(error) => return Err(io_error(error)),
        }
        let mut file = super::archive::open_guarded(path, false)?;
        let mut existing = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut existing)
            .map_err(io_error)?;
        if existing != bytes {
            return Err(failure(
                "BackupConflict",
                "Original configuration backup differs",
            ));
        }
        Ok(BackupGuard {
            _file: file,
            _parents: parents,
        })
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, HostError> {
    read_config(path, MAX_CONFIG_BYTES)
}

fn empty() -> Result<RegistrySnapshot, HostError> {
    Ok(RegistrySnapshot {
        version: 3,
        host_id: new_id()?,
        registry_rev: 0,
        settings: HostSettings::default(),
        devices: Vec::new(),
        setups: Vec::new(),
        drafts: Vec::new(),
        tombstones: Vec::new(),
        setup_tombstones: Vec::new(),
    })
}

pub(crate) fn identity_valid(identity: &Value) -> bool {
    identity.as_object().is_some_and(|items| {
        !items.is_empty()
            && items.iter().all(|(key, value)| {
                matches!(
                    key.as_str(),
                    "manufacturer"
                        | "model"
                        | "serial"
                        | "transport_serial"
                        | "operator_binding"
                        | "firmware"
                        | "head_model"
                        | "head_serial"
                        | "resource"
                        | "identity_quality"
                        | "identity_strength"
                        | "quality"
                        | "protocol"
                ) && value.as_str().is_some_and(|item| text(item, 256))
            })
    })
}
fn policy_valid(policy: &CheckPolicy, device: &DeviceRecord) -> bool {
    policy.interval_s >= 10
        && [policy.enumeration.as_ref(), policy.readonly.as_ref()]
            .iter()
            .all(|binding| {
                binding.map_or(true, |binding| {
                    binding.mode == device.verified_mode
                        && binding.config_rev == device.config_rev
                        && binding.profile_id == device.profile_id
                        && binding.probe_version > 0
                        && binding.identity == device.expected_identity
                })
            })
}

pub(crate) fn validate_snapshot(snapshot: &RegistrySnapshot) -> Result<(), HostError> {
    if snapshot.version != 3 || !valid_id(&snapshot.host_id) || snapshot.registry_rev > MAX_SEQUENCE
    {
        return Err(failure(
            "ConfigInvalid",
            "Invalid registry identity or revision",
        ));
    }
    if snapshot.devices.len() > 64
        || snapshot.setups.len() > 16
        || snapshot
            .drafts
            .iter()
            .filter(|draft| draft.status != "Cancelled")
            .count()
            > 64
        || snapshot.devices.len()
            + snapshot
                .drafts
                .iter()
                .filter(|draft| draft.status != "Cancelled")
                .count()
            > 64
        || snapshot.drafts.len() > 4160
        || snapshot.tombstones.len() > 4096
        || snapshot.setup_tombstones.len() > 4096
    {
        return Err(failure("ConfigCapacity", "Registry capacity exceeded"));
    }
    if !text(&snapshot.settings.host_name, 128) {
        return Err(failure("ConfigInvalid", "Invalid Host settings"));
    }
    if snapshot.settings.data_root.as_ref().is_some_and(|root| {
        !text(root, 1024)
            || !Path::new(root).is_absolute()
            || Path::new(root).components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
    }) {
        return Err(failure(
            "ConfigInvalid",
            "Data root must be an absolute normalized directory",
        ));
    }
    let catalog = Catalog::load(DOCUMENT)?;
    let mut ids = BTreeSet::new();
    let validate_device = |device: &DeviceRecord| -> Result<(), HostError> {
        let model = catalog.model(&device.model_id)?;
        if !valid_id(&device.device_id)
            || !text(&device.name, 128)
            || device.config_rev == 0
            || device.config_rev > MAX_SEQUENCE
            || model.driver_id != device.driver_id
            || model.category != device.category
            || !identity_valid(&device.expected_identity)
            || !matches!(
                device.verified_mode.as_str(),
                "real" | "simulate" | "unverified"
            )
            || !matches!(
                device.identity_strength.as_str(),
                "strong" | "operator_bound" | "transport_bound"
            )
            || !policy_valid(&device.check_policy, device)
        {
            return Err(failure("ConfigInvalid", "Invalid device record"));
        }
        catalog.validate_connection(&device.model_id, &device.profile_id, &device.params)?;
        if device.model_id=="tlb6700" {super::laser::validate_params(device.expected_identity["head_model"].as_str().unwrap_or(""),&device.params)?;}
        Ok(())
    };
    for device in &snapshot.devices {
        validate_device(device)?;
        if device.verified_mode != "real" {
            return Err(failure(
                "ConfigInvalid",
                "Active device lacks real verification",
            ));
        }
        if !ids.insert(&device.device_id) {
            return Err(failure("ConfigInvalid", "Duplicate device identity"));
        }
    }
    for tombstone in &snapshot.tombstones {
        validate_device(&tombstone.record)?;
        if !ids.insert(&tombstone.record.device_id)
            || tombstone.retired_rev > snapshot.registry_rev
            || tombstone.release_attempt_id.is_empty()
        {
            return Err(failure("ConfigInvalid", "Invalid retirement evidence"));
        }
    }
    for draft in &snapshot.drafts {
        if !valid_id(&draft.device_id)
            || !ids.insert(&draft.device_id)
            || !text(&draft.name, 128)
            || draft.revision == 0
            || draft.revision > MAX_SEQUENCE
            || !draft.params.is_object()
            || !matches!(draft.mode.as_str(), "unverified" | "real")
            || !matches!(
                draft.status.as_str(),
                "Imported draft" | "Draft" | "Cancelled"
            )
        {
            return Err(failure("ConfigInvalid", "Invalid draft"));
        }
    }
    let mut setup_ids = BTreeSet::new();
    let mut assigned = BTreeSet::new();
    for t in &snapshot.setup_tombstones {
        if !valid_id(&t.record.setup_id)
            || !setup_ids.insert(&t.record.setup_id)
            || !text(&t.record.name, 128)
            || t.record.kind != "fiber"
            || t.record.config_rev == 0
            || t.retired_rev > snapshot.registry_rev
            || !text(&t.release_attempt_id, 128)
            || !(1..=2).contains(&t.record.members.len())
            || t.record.members.iter().any(|id| !valid_id(id))
        {
            return Err(failure("SetupInvalid", "Invalid retired setup evidence"));
        }
    }
    for setup in &snapshot.setups {
        if !valid_id(&setup.setup_id)
            || !setup_ids.insert(&setup.setup_id)
            || !text(&setup.name, 128)
            || setup.kind != "fiber"
            || setup.config_rev == 0
            || setup.config_rev > MAX_SEQUENCE
            || !(1..=2).contains(&setup.members.len())
        {
            return Err(failure("SetupInvalid", "Invalid Fiber setup"));
        }
        let mut serials = BTreeSet::new();
        for member in &setup.members {
            let device = snapshot
                .devices
                .iter()
                .find(|device| &device.device_id == member)
                .ok_or_else(|| failure("SetupInvalid", "Setup member is not registered"))?;
            let serial = device
                .expected_identity
                .get("serial")
                .and_then(Value::as_str);
            if device.model_id != "mdt693b"
                || !matches!(serial, Some("2110148249-10" | "160721175410"))
                || !serials.insert(serial)
                || !assigned.insert(member)
            {
                return Err(failure(
                    "SetupInvalid",
                    "Controller side is unknown or already assigned",
                ));
            }
        }
    }
    Ok(())
}

pub struct Registry {
    path: PathBuf,
    current: Option<RegistrySnapshot>,
    blocked: Option<String>,
    store: Box<dyn AtomicStore>,
}
// Legacy identity/consent is never upgraded. Editable connection information is
// imported instead, with the complete original retained in an immutable backup.
fn import_nonreal(snapshot: &mut RegistrySnapshot) -> Result<bool, HostError> {
    let mut changed = false;
    let mut kept = Vec::new();
    for mut record in snapshot.devices.drain(..) {
        if record.verified_mode == "real" {
            for binding in [
                &mut record.check_policy.enumeration,
                &mut record.check_policy.readonly,
            ] {
                if binding
                    .as_ref()
                    .is_some_and(|binding| binding.mode != "real")
                {
                    *binding = None;
                    changed = true;
                }
            }
            kept.push(record);
        } else {
            changed = true;
            snapshot.drafts.push(DraftRecord {
                device_id: record.device_id,
                name: record.name,
                model_id: Some(record.model_id),
                profile_id: Some(record.profile_id),
                params: record.params,
                config_digest: String::new(),
                revision: record.config_rev,
                mode: "unverified".into(),
                status: "Imported draft".into(),
            });
        }
    }
    snapshot.devices = kept;
    for draft in &mut snapshot.drafts {
        if !matches!(draft.mode.as_str(), "real" | "unverified") {
            changed = true;
            draft.mode = "unverified".into();
            draft.config_digest.clear();
            if draft.status != "Cancelled" {
                draft.status = "Imported draft".into();
            }
        }
    }
    if changed {
        snapshot.setups.retain(|setup| {
            setup.members.iter().all(|id| {
                snapshot
                    .devices
                    .iter()
                    .any(|device| &device.device_id == id)
            })
        });
        snapshot.registry_rev = snapshot
            .registry_rev
            .checked_add(1)
            .filter(|rev| *rev <= MAX_SEQUENCE)
            .ok_or_else(|| failure("ConfigCapacity", "Registry revision exhausted"))?;
    }
    Ok(changed)
}
impl Registry {
    pub fn open(path: &Path) -> Result<Self, HostError> {
        Self::open_with_store(path, Box::new(FileStore))
    }
    fn open_with_store(path: &Path, store: Box<dyn AtomicStore>) -> Result<Self, HostError> {
        let mut registry = Self {
            path: path.to_path_buf(),
            current: None,
            blocked: None,
            store,
        };
        let original = match read_bounded(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.code == "ConfigStorage" && !path.exists() => None,
            Err(error) => {
                registry.blocked = Some(error.message);
                return Ok(registry);
            }
        };
        let snapshot = if let Some(bytes) = original {
            let value = match crate::runtime::strict_json(&bytes) {
                Ok(value) => value,
                Err(error) => {
                    registry.blocked = Some(error);
                    return Ok(registry);
                }
            };
            let version = value.get("version").and_then(Value::as_u64);
            match version {
                Some(1) => {
                    let migration = match migrate_v1(&bytes) {
                        Ok(migration) => migration,
                        Err(error) => {
                            registry.blocked = Some(error.message);
                            return Ok(registry);
                        }
                    };
                    let _backup = registry
                        .store
                        .backup(&backup_path(path), &migration.backup_bytes)?;
                    let mut snapshot = empty()?;
                    snapshot.settings = migration.settings;
                    snapshot.drafts = migration.drafts;
                    registry.persist(&snapshot)?;
                    snapshot
                }
                Some(2 | 3) => match serde_json::from_slice::<RegistrySnapshot>(&bytes) {
                    Ok(mut snapshot) => {
                        let legacy = snapshot.version == 2;
                        snapshot.version = 3;
                        if import_nonreal(&mut snapshot)? || legacy {
                            // Validate before backup/write: malformed imports never
                            // replace the user's original registry.
                            if let Err(error) = validate_snapshot(&snapshot) {
                                registry.blocked = Some(error.message);
                                return Ok(registry);
                            }
                            let _backup = registry.store.backup(
                                &path.with_extension(if legacy {
                                    "pre-rust.json"
                                } else {
                                    "pre-real-only.json"
                                }),
                                &bytes,
                            )?;
                            registry.persist(&snapshot)?;
                        }
                        snapshot
                    }
                    Err(error) => {
                        registry.blocked = Some(error.to_string());
                        return Ok(registry);
                    }
                },
                _ => {
                    registry.blocked =
                        Some("Configuration version is corrupt or unsupported".into());
                    return Ok(registry);
                }
            }
        } else {
            let snapshot = empty()?;
            registry.persist(&snapshot)?;
            snapshot
        };
        match validate_snapshot(&snapshot) {
            Ok(()) => registry.current = Some(snapshot),
            Err(error) => registry.blocked = Some(error.message),
        }
        Ok(registry)
    }
    pub fn snapshot(&self) -> Result<RegistrySnapshot, HostError> {
        self.current.clone().ok_or_else(|| {
            failure(
                "ConfigReadOnly",
                self.blocked
                    .as_deref()
                    .unwrap_or("Configuration unavailable"),
            )
        })
    }
    pub fn blocked_reason(&self) -> Option<&str> {
        self.blocked.as_deref()
    }
    fn persist(&self, snapshot: &RegistrySnapshot) -> Result<(), HostError> {
        validate_snapshot(snapshot)?;
        let bytes = serde_json::to_vec_pretty(snapshot)
            .map_err(|error| failure("ConfigInvalid", error.to_string()))?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(failure("ConfigCapacity", "Configuration exceeds 1 MiB"));
        }
        self.store.write(&self.path, &bytes)
    }
    pub fn commit(
        &mut self,
        expected_rev: u64,
        change: RegistryChange,
    ) -> Result<RegistrySnapshot, HostError> {
        let mut next = self.snapshot()?;
        if expected_rev != next.registry_rev {
            return Err(failure("ConfigConflict", "Registry revision changed"));
        }
        next.registry_rev = next
            .registry_rev
            .checked_add(1)
            .filter(|rev| *rev <= MAX_SEQUENCE)
            .ok_or_else(|| failure("ConfigCapacity", "Registry revision exhausted"))?;
        match change {
            RegistryChange::SaveDraft(draft) => {
                if next
                    .devices
                    .iter()
                    .any(|value| value.device_id == draft.device_id)
                    || next
                        .tombstones
                        .iter()
                        .any(|value| value.record.device_id == draft.device_id)
                {
                    return Err(failure("IdentityRetired", "Draft identity is already used"));
                }
                if let Some(current) = next
                    .drafts
                    .iter_mut()
                    .find(|value| value.device_id == draft.device_id)
                {
                    if current.status == "Cancelled" || draft.revision != current.revision + 1 {
                        return Err(failure(
                            "ConfigConflict",
                            "Draft identity retired or revision changed",
                        ));
                    }
                    *current = draft;
                } else {
                    if draft.revision != 1 {
                        return Err(failure("ConfigConflict", "New draft revision must be one"));
                    }
                    next.drafts.push(draft);
                }
            }
            RegistryChange::CancelDraft {
                device_id,
                config_rev,
                permit,
            } => {
                let draft = next
                    .drafts
                    .iter_mut()
                    .find(|value| value.device_id == device_id)
                    .ok_or_else(|| failure("DraftUnknown", "Draft not found"))?;
                if draft.status == "Cancelled"
                    || draft.revision != config_rev
                    || permit.device_id != device_id
                    || permit.config_rev != config_rev
                    || permit.attempt_id.is_empty()
                {
                    return Err(failure(
                        "ReleaseRequired",
                        "Current draft release evidence required",
                    ));
                }
                draft.status = "Cancelled".into();
            }
            RegistryChange::SaveDevice(mut device) => {
                if next
                    .tombstones
                    .iter()
                    .any(|item| item.record.device_id == device.device_id)
                {
                    return Err(failure(
                        "IdentityRetired",
                        "Device identity cannot be reused",
                    ));
                }
                if next
                    .devices
                    .iter()
                    .any(|item| item.device_id == device.device_id)
                {
                    return Err(failure(
                        "ConfigConflict",
                        "Physical rebind requires a new identity",
                    ));
                }
                if device.config_rev != 1 {
                    return Err(failure("ConfigConflict", "New device revision must be one"));
                }
                device.params = Catalog::load(DOCUMENT)?.validate_connection(
                    &device.model_id,
                    &device.profile_id,
                    &device.params,
                )?;
                next.drafts
                    .retain(|draft| draft.device_id != device.device_id);
                next.devices.push(device);
            }
            RegistryChange::EditName {
                device_id,
                config_rev,
                name,
            } => {
                let device = next
                    .devices
                    .iter_mut()
                    .find(|item| item.device_id == device_id)
                    .ok_or_else(|| failure("DeviceUnknown", "Device is not registered"))?;
                if device.config_rev != config_rev {
                    return Err(failure("ConfigConflict", "Device revision changed"));
                }
                device.config_rev += 1;
                device.name = name;
                device.check_policy = CheckPolicy::default();
            }
            RegistryChange::LaserLimits {device_id,config_rev,limits} => {
                let device=next.devices.iter_mut().find(|d|d.device_id==device_id)
                    .ok_or_else(||failure("DeviceUnknown","Device is not registered"))?;
                if device.config_rev!=config_rev||device.model_id!="tlb6700" {return Err(failure("ConfigConflict","Laser configuration changed"));}
                let narrowed=super::laser::validate_limits(device.expected_identity["head_model"].as_str().unwrap_or(""),&limits)?;
                let params=device.params.as_object_mut().ok_or_else(||failure("ConfigInvalid","Invalid laser parameters"))?;
                for (key,value) in narrowed.as_object().unwrap() {params.insert(key.clone(),value.clone());}
                device.config_rev+=1;
                device.check_policy.enumeration=None;device.check_policy.readonly=None;
            }
            RegistryChange::RetireDevice {
                device_id,
                config_rev,
                permit,
            } => {
                let index = next
                    .devices
                    .iter()
                    .position(|item| item.device_id == device_id)
                    .ok_or_else(|| failure("DeviceUnknown", "Device is not registered"))?;
                if config_rev != next.devices[index].config_rev
                    || permit.device_id != device_id
                    || permit.config_rev != config_rev
                    || permit.attempt_id.is_empty()
                {
                    return Err(failure(
                        "ReleaseRequired",
                        "Current confirmed-release evidence is required",
                    ));
                }
                if next
                    .setups
                    .iter()
                    .any(|setup| setup.members.contains(&device_id))
                {
                    return Err(failure(
                        "SetupRetained",
                        "Detach setup responsibility before retiring member",
                    ));
                }
                next.tombstones.push(Tombstone {
                    record: next.devices.remove(index),
                    retired_rev: next.registry_rev,
                    release_attempt_id: permit.attempt_id,
                });
            }
            RegistryChange::SaveSetup(setup) => {
                if next
                    .setup_tombstones
                    .iter()
                    .any(|t| t.record.setup_id == setup.setup_id)
                {
                    return Err(failure(
                        "IdentityRetired",
                        "Setup identity cannot be reused",
                    ));
                }
                if next
                    .setups
                    .iter()
                    .any(|item| item.setup_id == setup.setup_id)
                    || setup.config_rev != 1
                {
                    return Err(failure(
                        "ConfigConflict",
                        "Setup rebind requires new identity and prior release",
                    ));
                }
                next.setups.push(setup);
            }
            RegistryChange::RetireSetup {
                setup_id,
                config_rev,
                permit,
            } => {
                let index = next
                    .setups
                    .iter()
                    .position(|s| s.setup_id == setup_id)
                    .ok_or_else(|| failure("SetupUnknown", "Setup not found"))?;
                if next.setups[index].config_rev != config_rev
                    || permit.device_id != setup_id
                    || permit.config_rev != config_rev
                    || permit.attempt_id.is_empty()
                {
                    return Err(failure(
                        "ReleaseRequired",
                        "Current confirmed-release evidence is required",
                    ));
                }
                next.setup_tombstones.push(SetupTombstone {
                    record: next.setups.remove(index),
                    retired_rev: next.registry_rev,
                    release_attempt_id: permit.attempt_id,
                });
            }
            RegistryChange::SaveSettings(settings) => next.settings = settings,
            RegistryChange::SaveCheckPolicy {
                device_id,
                config_rev,
                policy,
            } => {
                let device = next
                    .devices
                    .iter_mut()
                    .find(|d| d.device_id == device_id)
                    .ok_or_else(|| failure("DeviceUnknown", "Device is not registered"))?;
                if device.config_rev != config_rev {
                    return Err(failure("ConfigConflict", "Device revision changed"));
                }
                device.check_policy = policy;
            }
        }
        self.persist(&next)?;
        self.current = Some(next.clone());
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::contracts::{CheckPolicy, DeviceRecord, HostSettings, SetupRecord};
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "yang-registry-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join("registry.json")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn device(number: u64) -> DeviceRecord {
        DeviceRecord {
            device_id: format!("{number:032x}"),
            name: format!("Gain {number}"),
            category: "Custom".into(),
            model_id: "gain".into(),
            driver_id: "gain".into(),
            profile_id: "cp210x-serial".into(),
            params: json!({"port":format!("COM{number}")}),
            expected_identity: json!({"model":"Gain Chip Driver"}),
            identity_strength: "operator_bound".into(),
            verified_mode: "real".into(),
            config_rev: 1,
            check_policy: CheckPolicy::default(),
        }
    }
    #[test]
    fn native_identity_metadata_is_bounded_but_preserves_strong_and_weak_claims() {
        assert!(identity_valid(&json!({"model":"8-channel voltage source","resource":"COM4","identity_strength":"weak","protocol":"continuous_8ch_telemetry"})));
        assert!(identity_valid(&json!({"model":"MDT693B","serial":"2110148249-10","resource":"COM5","quality":"strong_identity"})));
        assert!(!identity_valid(&json!({"model":"Gain Chip Driver","protocol":{"command":"raw"}})));
        assert!(!identity_valid(&json!({"model":"Gain Chip Driver","unreviewed":"arbitrary"})));
    }
    #[test]
    fn laser_limits_persist_without_rebinding_and_reject_widening() {
        let directory=Directory::new();
        let mut registry=Registry::open(&directory.path()).unwrap();
        let mut laser=device(55);
        laser.name="Laser".into();laser.category="Laser".into();laser.model_id="tlb6700".into();
        laser.driver_id="tlb6700".into();laser.profile_id="newport-usb".into();
        laser.params=json!({"device_key":"6700 SN1012"});
        laser.expected_identity=json!({"manufacturer":"New Focus","model":"TLB-6700","serial":"1012","firmware":"2.4","head_model":"6722-P","head_serial":"P1001"});
        laser.identity_strength="strong".into();
        registry.commit(0,RegistryChange::SaveDevice(laser.clone())).unwrap();
        let saved=registry.commit(1,RegistryChange::LaserLimits {device_id:laser.device_id.clone(),config_rev:1,
            limits:json!({"min_nm":1050.0,"max_nm":1080.0,"max_speed_nm_s":1.0})}).unwrap();
        assert_eq!(saved.devices[0].expected_identity,laser.expected_identity);
        assert_eq!(saved.devices[0].params["device_key"],laser.params["device_key"]);
        assert_eq!(saved.devices[0].config_rev,2);
        assert_eq!(Registry::open(&directory.path()).unwrap().snapshot().unwrap(),saved);
        let bytes=std::fs::read(directory.path()).unwrap();
        assert!(registry.commit(2,RegistryChange::LaserLimits {device_id:laser.device_id,config_rev:2,
            limits:json!({"min_nm":1044.0,"max_nm":1080.0,"max_speed_nm_s":1.0})}).is_err());
        assert_eq!(registry.snapshot().unwrap(),saved);
        assert_eq!(std::fs::read(directory.path()).unwrap(),bytes);
    }
    struct Failure(&'static str);
    impl AtomicStore for Failure {
        fn write(&self, _: &std::path::Path, _: &[u8]) -> Result<(), HostError> {
            Err(HostError::new(self.0, "Injected storage failure"))
        }
        fn backup(&self, _: &std::path::Path, _: &[u8]) -> Result<BackupGuard, HostError> {
            Err(HostError::new(self.0, "Injected backup failure"))
        }
    }
    #[test]
    fn real_only_migration_preserves_real_devices_and_imports_legacy_parameters() {
        let directory = Directory::new();
        let mut initial = empty().unwrap();
        initial.registry_rev = 7;
        let actual = device(4);
        let mut legacy = device(5);
        legacy.verified_mode = "simulate".into();
        legacy.expected_identity = json!({"model":"Gain Chip Driver","serial":"SIMULATED"});
        initial.devices = vec![actual.clone(), legacy.clone()];
        let original = serde_json::to_vec_pretty(&initial).unwrap();
        std::fs::write(directory.path(), &original).unwrap();
        let migrated = Registry::open(&directory.path())
            .unwrap()
            .snapshot()
            .unwrap();
        assert_eq!(migrated.devices, vec![actual]);
        assert_eq!(migrated.drafts.len(), 1);
        assert_eq!(migrated.drafts[0].device_id, legacy.device_id);
        assert_eq!(migrated.drafts[0].params, json!({"port":"COM5"}));
        assert_eq!(migrated.drafts[0].mode, "unverified");
        assert_eq!(migrated.drafts[0].status, "Imported draft");
        assert_eq!(migrated.registry_rev, 8);
        assert_eq!(
            std::fs::read(directory.path().with_extension("pre-real-only.json")).unwrap(),
            original
        );
        let again = Registry::open(&directory.path())
            .unwrap()
            .snapshot()
            .unwrap();
        assert_eq!(again, migrated);
    }
    #[test]
    fn real_only_migration_revokes_legacy_checks_without_discarding_real_device() {
        let directory = Directory::new();
        let mut initial = empty().unwrap();
        let mut actual = device(4);
        actual.check_policy.enumeration = Some(CheckBinding {
            mode: "simulate".into(),
            identity: actual.expected_identity.clone(),
            config_rev: actual.config_rev,
            profile_id: actual.profile_id.clone(),
            probe_version: 1,
        });
        initial.devices.push(actual.clone());
        let original = serde_json::to_vec(&initial).unwrap();
        std::fs::write(directory.path(), &original).unwrap();
        let migrated = Registry::open(&directory.path())
            .unwrap()
            .snapshot()
            .unwrap();
        actual.check_policy.enumeration = None;
        assert_eq!(migrated.devices, vec![actual]);
        assert_eq!(migrated.registry_rev, 1);
        assert_eq!(
            std::fs::read(directory.path().with_extension("pre-real-only.json")).unwrap(),
            original
        );
    }
    #[test]
    fn migration_preserves_original_bytes() {
        let original = b"{\n \"version\":1,\"python_path\":\"VISA/python.exe\",\"bindings\":{\"gain\":\"COM3\",\"osa\":null}}\n";
        let migration = migrate_v1(original).unwrap();
        assert_eq!(migration.backup_bytes, original);
        assert!(migration.devices.is_empty());
        assert_eq!(migration.drafts.len(), 1);
        assert_eq!(migration.drafts[0].status, "Imported draft");
        let directory = Directory::new();
        std::fs::write(directory.path(), original).unwrap();
        let registry = Registry::open(&directory.path()).unwrap();
        assert!(registry.snapshot().unwrap().devices.is_empty());
        assert_eq!(
            std::fs::read(backup_path(&directory.path())).unwrap(),
            original
        );
    }
    #[test]
    fn future_version_blocks_writes() {
        let directory = Directory::new();
        let original = b"{\"version\":99,\"devices\":[\"untouched\"]}";
        std::fs::write(directory.path(), original).unwrap();
        let mut registry = Registry::open(&directory.path()).unwrap();
        assert_eq!(
            registry
                .commit(0, RegistryChange::SaveSettings(HostSettings::default()))
                .unwrap_err()
                .code,
            "ConfigReadOnly"
        );
        assert!(registry.snapshot().is_err());
        assert_eq!(std::fs::read(directory.path()).unwrap(), original);
    }
    #[test]
    fn native_worker_registry_v2_and_preferences_migrate_without_identity_loss() {
        let directory = Directory::new();
        let mut initial = empty().unwrap();
        initial.version = 2;
        initial.registry_rev = 7;
        initial.settings.host_name = "Yang Lab 台子".into();
        initial.devices.push(device(3));
        let mut value = serde_json::to_value(&initial).unwrap();
        value["settings"]["python_path"] = serde_json::json!("C:/obsolete/python.exe");
        let original = serde_json::to_vec(&value).unwrap();
        std::fs::write(directory.path(), &original).unwrap();
        let migrated = Registry::open(&directory.path())
            .unwrap()
            .snapshot()
            .unwrap();
        assert_eq!(migrated.version, 3);
        assert_eq!(migrated.host_id, initial.host_id);
        assert_eq!(migrated.registry_rev, 7);
        assert_eq!(migrated.devices, initial.devices);
        let serialized = serde_json::to_value(&migrated).unwrap();
        assert!(serialized["settings"].get("python_path").is_none());
        assert_eq!(
            std::fs::read(directory.path().with_extension("pre-rust.json")).unwrap(),
            original
        );
    }
    #[test]
    fn linked_backup_cannot_destroy_original_registry_bytes() {
        let directory = Directory::new();
        let mut initial = empty().unwrap();
        initial.version = 2;
        let original = serde_json::to_vec(&initial).unwrap();
        std::fs::write(directory.path(), &original).unwrap();
        let alias = directory.0.join("linked");
        junction(&alias, &directory.0);
        let result = backup_original(&alias.join("registry.json"), &original);
        std::fs::remove_dir(&alias).unwrap();
        assert!(result.is_err(), "linked backup accepted");
        assert_eq!(std::fs::read(directory.path()).unwrap(), original);
    }
    #[test]
    fn linked_source_cannot_be_migrated_or_overwritten() {
        let directory = Directory::new();
        let mut initial = empty().unwrap();
        initial.version = 2;
        let original = serde_json::to_vec(&initial).unwrap();
        std::fs::write(directory.path(), &original).unwrap();
        let alias = directory.0.join("linked");
        junction(&alias, &directory.0);
        let result = read_bounded(&alias.join("registry.json"));
        std::fs::remove_dir(&alias).unwrap();
        assert!(result.is_err(), "linked source accepted");
        assert_eq!(std::fs::read(directory.path()).unwrap(), original);
        assert!(!directory.path().with_extension("pre-rust.json").exists());
    }
    fn junction(link: &Path, target: &Path) {
        let output = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[test]
    fn native_worker_corrupt_or_future_config_is_not_overwritten() {
        let directory = Directory::new();
        for original in [b"{broken".as_slice(), b"{\"version\":99}".as_slice()] {
            std::fs::write(directory.path(), original).unwrap();
            let mut registry = Registry::open(&directory.path()).unwrap();
            assert!(registry.snapshot().is_err());
            assert!(registry
                .commit(0, RegistryChange::SaveSettings(HostSettings::default()))
                .is_err());
            assert_eq!(std::fs::read(directory.path()).unwrap(), original);
        }
    }
    #[test]
    fn native_worker_duplicate_parameters_never_migrate() {
        let directory = Directory::new();
        let mut initial = empty().unwrap();
        initial.version = 2;
        initial.devices.push(device(3));
        let text = serde_json::to_string(&initial).unwrap().replace(
            "\"params\":{\"port\":\"COM3\"}",
            "\"params\":{\"port\":\"COM3\",\"port\":\"COM4\"}",
        );
        assert!(text.contains("\"port\":\"COM3\",\"port\":\"COM4\""));
        std::fs::write(directory.path(), text.as_bytes()).unwrap();
        assert!(Registry::open(&directory.path())
            .unwrap()
            .snapshot()
            .is_err());
        assert_eq!(std::fs::read(directory.path()).unwrap(), text.as_bytes());
        assert!(!directory.path().with_extension("pre-rust.json").exists());
    }
    #[test]
    fn concurrent_edit_conflicts() {
        let directory = Directory::new();
        let mut registry = Registry::open(&directory.path()).unwrap();
        let saved = registry
            .commit(0, RegistryChange::SaveDevice(device(1)))
            .unwrap();
        let stale = saved.registry_rev;
        registry
            .commit(
                stale,
                RegistryChange::EditName {
                    device_id: device(1).device_id,
                    config_rev: 1,
                    name: "Renamed".into(),
                },
            )
            .unwrap();
        assert_eq!(
            registry
                .commit(stale, RegistryChange::SaveDevice(device(2)))
                .unwrap_err()
                .code,
            "ConfigConflict"
        );
        let updated = registry.snapshot().unwrap();
        assert_eq!(updated.devices[0].device_id, device(1).device_id);
        assert_eq!(updated.devices[0].config_rev, 2);
    }
    #[test]
    fn atomic_replace_failure_publishes_nothing() {
        let directory = Directory::new();
        let mut registry = Registry::open(&directory.path()).unwrap();
        let before = registry.snapshot().unwrap();
        let original = std::fs::read(directory.path()).unwrap();
        registry.store = Box::new(Failure("AtomicReplaceFailed"));
        assert_eq!(
            registry
                .commit(0, RegistryChange::SaveDevice(device(1)))
                .unwrap_err()
                .code,
            "AtomicReplaceFailed"
        );
        assert_eq!(registry.snapshot().unwrap(), before);
        assert_eq!(std::fs::read(directory.path()).unwrap(), original);
    }
    #[test]
    fn disk_full_and_backup_failure_preserve_original_configuration() {
        let directory = Directory::new();
        let mut registry = Registry::open(&directory.path()).unwrap();
        let before = registry.snapshot().unwrap();
        registry.store = Box::new(Failure("DiskFull"));
        assert_eq!(
            registry
                .commit(0, RegistryChange::SaveDevice(device(1)))
                .unwrap_err()
                .code,
            "DiskFull"
        );
        assert_eq!(registry.snapshot().unwrap(), before);
        let original = b"{\"version\":1,\"bindings\":{\"gain\":\"COM4\"}}";
        std::fs::write(directory.path(), original).unwrap();
        assert!(
            Registry::open_with_store(&directory.path(), Box::new(Failure("BackupFailed")))
                .is_err()
        );
        assert_eq!(std::fs::read(directory.path()).unwrap(), original);
    }
    #[test]
    fn registration_and_setup_limits_and_dangling_members_are_rejected() {
        let directory = Directory::new();
        let mut registry = Registry::open(&directory.path()).unwrap();
        for number in 1..=64 {
            let revision = registry.snapshot().unwrap().registry_rev;
            registry
                .commit(revision, RegistryChange::SaveDevice(device(number)))
                .unwrap();
        }
        assert_eq!(
            registry
                .commit(64, RegistryChange::SaveDevice(device(65)))
                .unwrap_err()
                .code,
            "ConfigCapacity"
        );
        let setup = SetupRecord {
            setup_id: "a".repeat(32),
            name: "Fiber".into(),
            kind: "fiber".into(),
            members: vec![device(1).device_id],
            config_rev: 1,
        };
        assert_eq!(
            registry
                .commit(64, RegistryChange::SaveSetup(setup))
                .unwrap_err()
                .code,
            "SetupInvalid"
        );
        let mut source = registry.snapshot().unwrap();
        source.setups = (1..=17)
            .map(|number| SetupRecord {
                setup_id: format!("{number:032x}"),
                name: "Fiber".into(),
                kind: "fiber".into(),
                members: vec!["f".repeat(32)],
                config_rev: 1,
            })
            .collect();
        assert!(validate_snapshot(&source).is_err());
    }
    #[test]
    fn retiring_requires_release_and_does_not_allow_identity_reuse() {
        let directory = Directory::new();
        let mut registry = Registry::open(&directory.path()).unwrap();
        registry
            .commit(0, RegistryChange::SaveDevice(device(1)))
            .unwrap();
        let permit = ReleasePermit::confirmed(device(1).device_id.clone(), 1, "cleanup-1".into());
        registry
            .commit(
                1,
                RegistryChange::RetireDevice {
                    device_id: device(1).device_id,
                    config_rev: 1,
                    permit,
                },
            )
            .unwrap();
        assert_eq!(
            registry
                .commit(2, RegistryChange::SaveDevice(device(1)))
                .unwrap_err()
                .code,
            "IdentityRetired"
        );
    }
    #[test]
    fn host_id_persists_and_runtime_status_is_not_a_configuration_field() {
        let directory = Directory::new();
        let registry = Registry::open(&directory.path()).unwrap();
        let first = registry.snapshot().unwrap();
        assert_eq!(first.host_id.len(), 32);
        assert_eq!(
            Registry::open(&directory.path())
                .unwrap()
                .snapshot()
                .unwrap()
                .host_id,
            first.host_id
        );
        let mut source = serde_json::to_value(first).unwrap();
        source["ONLINE"] = json!(true);
        std::fs::write(directory.path(), serde_json::to_vec(&source).unwrap()).unwrap();
        assert!(Registry::open(&directory.path())
            .unwrap()
            .snapshot()
            .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn actual_windows_replace_failure_keeps_snapshot_and_disk() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = Directory::new();
        let mut registry = Registry::open(&directory.path()).unwrap();
        let before = registry.snapshot().unwrap();
        let original = std::fs::read(directory.path()).unwrap();
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(directory.path())
            .unwrap();
        assert!(registry
            .commit(0, RegistryChange::SaveDevice(device(1)))
            .is_err());
        assert_eq!(registry.snapshot().unwrap(), before);
        drop(guard);
        assert_eq!(std::fs::read(directory.path()).unwrap(), original);
        assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn retired_setup_releases_members_without_reusing_its_identity() {
        let directory = Directory::new();
        let mut registry = Registry::open(&directory.path()).unwrap();
        let mut mdt = device(1);
        mdt.model_id = "mdt693b".into();
        mdt.driver_id = "mdt693b".into();
        mdt.category = "Piezo Controller".into();
        mdt.profile_id = "serial".into();
        mdt.expected_identity = json!({"serial":"2110148249-10"});
        registry
            .commit(0, RegistryChange::SaveDevice(mdt.clone()))
            .unwrap();
        let setup = SetupRecord {
            setup_id: "a".repeat(32),
            name: "Fiber".into(),
            kind: "fiber".into(),
            members: vec![mdt.device_id],
            config_rev: 1,
        };
        registry
            .commit(1, RegistryChange::SaveSetup(setup.clone()))
            .unwrap();
        let permit = ReleasePermit::confirmed(setup.setup_id.clone(), 1, "hold-release".into());
        let removed = registry
            .commit(
                2,
                RegistryChange::RetireSetup {
                    setup_id: setup.setup_id.clone(),
                    config_rev: 1,
                    permit,
                },
            )
            .unwrap();
        assert!(removed.setups.is_empty());
        assert_eq!(removed.devices.len(), 1);
        assert_eq!(
            registry
                .commit(3, RegistryChange::SaveSetup(setup))
                .unwrap_err()
                .code,
            "IdentityRetired"
        );
    }
}
