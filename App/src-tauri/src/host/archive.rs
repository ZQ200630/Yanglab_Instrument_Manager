//! Persistent native data, not a boot-bound result cache or command capability.

use super::{
    contracts::{valid_id, HostError, MAX_SEQUENCE},
    leases::DomainRef,
    registry::new_id,
    verification::sha256_bytes,
};
use crate::runtime::{strict_json, validate_capture_descriptor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_RECORDINGS: usize = 4096;
const MAX_DIRECTORY_ENTRIES: usize = 16384;
const MAX_MANIFEST: u64 = 16384;
const MAX_NATIVE: u64 = 200001 * 16;
const MAX_CHUNK: u64 = 16384;
fn error(e: impl std::fmt::Display) -> HostError {
    HostError::new("ArchiveUnavailable", e.to_string())
}
pub fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureOrigin {
    pub host_id: String,
    pub domain: DomainRef,
    pub device_identity: Value,
    pub config_rev: u64,
    pub operation_id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveRef {
    pub id: String,
    pub name: String,
    pub host_id: String,
    pub domain: DomainRef,
    pub byte_count: u64,
    pub sample_count: u64,
    pub sha256: String,
    pub metadata: Value,
}
impl ArchiveRef {
    pub(crate) fn validate(&self) -> Result<(), HostError> {
        self.domain.validate()?;
        if !valid_id(&self.host_id) || !valid_id(&self.id) || !valid_name(&self.name) {
            return Err(error("Invalid archive reference"));
        }
        validate_capture_descriptor(&json!({"schema":1,"kind":"osa_trace","capture_id":self.id,
            "point_count":self.sample_count,"byte_count":self.byte_count,"sha256":self.sha256,
            "metadata":self.metadata}))
        .map_err(error)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    source_kind: String,
    name: String,
    archived_at_unix_ms: u64,
    origin: CaptureOrigin,
    descriptor: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    schema: u32,
    source_kind: String,
    name: String,
    started_at_unix_ms: u64,
    origin: CaptureOrigin,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureFailure {
    schema: u32,
    phase: String,
    primary_error: String,
    hardware_read_completed: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Seal {
    schema: u32,
    manifest_bytes: u64,
    manifest_sha256: String,
    native_bytes: u64,
    native_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    schema: u32,
    host_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveEntry {
    pub id: String,
    pub name: String,
    pub state: String,
    pub reference: Option<ArchiveRef>,
    pub error: Option<String>,
}

fn ordinary(metadata: &fs::Metadata, directory: bool) -> Result<(), HostError> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(error("Archive reparse point forbidden"));
        }
    }
    if metadata.file_type().is_symlink()
        || metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
    {
        return Err(error("Archive requires ordinary files and directories"));
    }
    Ok(())
}
// Directory handles deny write/delete sharing for the entire transaction. This
// also pins ancestors: replacing a checked directory with a junction cannot
// redirect a later child open. Files are validated through the opened handle.
fn open_guarded(path: &Path, directory: bool) -> Result<File, HostError> {
    ordinary(&fs::symlink_metadata(path).map_err(error)?, directory)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
        };
        options.share_mode(FILE_SHARE_READ).custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        );
        if directory {
            options.access_mode(FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY);
        }
    }
    let file = options.open(path).map_err(error)?;
    ordinary(&file.metadata().map_err(error)?, directory)?;
    Ok(file)
}
fn pin_directories(path: &Path) -> Result<Vec<File>, HostError> {
    if !path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(error("Absolute normalized archive directory required"));
    }
    let mut ancestors: Vec<_> = path.ancestors().collect();
    ancestors.reverse();
    let mut guards = Vec::new();
    for ancestor in ancestors {
        guards.push(open_guarded(ancestor, true)?);
    }
    Ok(guards)
}
fn bounded_file(path: &Path, maximum: u64) -> Result<Vec<u8>, HostError> {
    let mut file = open_guarded(path, false)?;
    let length = file.metadata().map_err(error)?.len();
    if length > maximum {
        return Err(error("Archive file exceeds limit"));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    Read::by_ref(&mut file)
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 != length {
        return Err(error("Archive file length changed"));
    }
    Ok(bytes)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), HostError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(error)?;
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(error)
}
fn publish_seal(directory: &Path, seal: &[u8]) -> Result<(), HostError> {
    let temporary = directory.join(format!(".complete-{}.tmp", new_id()?));
    write_new(&temporary, seal)?;
    #[cfg(windows)]
    {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::{
            Wdk::Storage::FileSystem::{
                FileRenameInformation, NtSetInformationFile, FILE_RENAME_INFORMATION,
            },
            Win32::{
                Foundation::GENERIC_WRITE,
                Storage::FileSystem::{
                    DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ,
                },
                System::IO::IO_STATUS_BLOCK,
            },
        };
        // Same-parent native rename does not reopen the target parent for writing
        // (MoveFileExW does, conflicting with its strong read pin).
        let source = OpenOptions::new()
            .write(true)
            .access_mode(DELETE | GENERIC_WRITE)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)
            .open(&temporary)
            .map_err(error)?;
        ordinary(&source.metadata().map_err(error)?, false)?;
        let name: Vec<u16> = "complete.json".encode_utf16().collect();
        let size = std::mem::size_of::<FILE_RENAME_INFORMATION>() + (name.len() + 1) * 2;
        let mut buffer = vec![0_u64; (size + 7) / 8];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        unsafe {
            // Zeroed ReplaceIfExists is false: never overwrite an old seal.
            // NULL + simple name renames inside the source's own parent, not
            // relative to the working directory or an arbitrary destination.
            (*info).RootDirectory = std::ptr::null_mut();
            (*info).FileNameLength = (name.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                name.len(),
            );
        }
        let mut status = IO_STATUS_BLOCK::default();
        let result = unsafe {
            NtSetInformationFile(
                source.as_raw_handle(),
                &mut status,
                info.cast(),
                size as u32,
                FileRenameInformation,
            )
        };
        if result < 0 {
            return Err(error(format!("Seal publish NTSTATUS: {result:#x}")));
        }
        source.sync_all().map_err(error)?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(error("Archive sealing requires Windows"))
    }
}
fn validate_native(bytes: &[u8], descriptor: &Value) -> Result<(), HostError> {
    if bytes.len() as u64
        != descriptor["byte_count"]
            .as_u64()
            .ok_or_else(|| error("Missing native byte count"))?
        || sha256_bytes(bytes)? != descriptor["sha256"].as_str().unwrap_or("")
    {
        return Err(error("Native length or SHA256 mismatch"));
    }
    let mut previous = 0.0;
    for pair in bytes.chunks_exact(16) {
        let x = f64::from_le_bytes(pair[..8].try_into().unwrap());
        let y = f64::from_le_bytes(pair[8..].try_into().unwrap());
        if !x.is_finite()
            || x <= previous
            || !y.is_finite()
            || (descriptor["metadata"]["native_unit"] == "W" && y < 0.0)
        {
            return Err(error("Invalid native wavelength or power value"));
        }
        previous = x;
    }
    Ok(())
}

// Internal native-UI capability: neither this type nor its path is accepted by
// a Host RPC. Hold ordinary directory handles throughout selection/download/write.
pub(crate) struct SelectedDirectory {
    path: PathBuf,
    _pins: Vec<File>,
}
impl SelectedDirectory {
    pub(crate) fn open(path: &Path) -> Result<Self, HostError> {
        let pins = pin_directories(path)?;
        Ok(Self {
            path: path.to_owned(),
            _pins: pins,
        })
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

pub(crate) struct NativeExport {
    manifest: Manifest,
    manifest_bytes: Vec<u8>,
    native: Vec<u8>,
}
impl NativeExport {
    /// Native diagnostic export retains the exact source bytes, not a CSV
    /// reconstruction. Both files remain create-new and durably synced.
    pub(crate) fn write_raw(self, selected: &SelectedDirectory) -> Result<(), HostError> {
        write_new(&selected.path.join("manifest.json"), &self.manifest_bytes)?;
        write_new(&selected.path.join("native.bin"), &self.native)?;
        Ok(())
    }
    pub(crate) fn verify(
        reference: &ArchiveRef,
        manifest_bytes: Vec<u8>,
        native: Vec<u8>,
    ) -> Result<Self, HostError> {
        reference.validate()?;
        if manifest_bytes.len() as u64 > MAX_MANIFEST {
            return Err(error("Manifest exceeds limit"));
        }
        let manifest: Manifest =
            serde_json::from_value(strict_json(&manifest_bytes).map_err(error)?).map_err(error)?;
        validate_origin(&manifest.origin, &reference.host_id)?;
        validate_capture_descriptor(&manifest.descriptor).map_err(error)?;
        if manifest.schema != 1
            || manifest.source_kind != "real"
            || manifest.archived_at_unix_ms == 0
            || ArchiveStore::reference(&manifest) != *reference
        {
            return Err(error("Export reference or provenance mismatch"));
        }
        validate_native(&native, &manifest.descriptor)?;
        Ok(Self {
            manifest,
            manifest_bytes,
            native,
        })
    }
    pub(crate) fn write(self, selected: &SelectedDirectory) -> Result<PathBuf, HostError> {
        let directory = selected
            .path
            .join(format!("{}-{}", self.manifest.name, new_id()?));
        fs::create_dir(&directory).map_err(error)?;
        let _directory = pin_directories(&directory)?;
        let unit = self.manifest.descriptor["metadata"]["native_unit"]
            .as_str()
            .unwrap();
        let mut csv = format!("wavelength_nm,power_{unit}\n");
        for pair in self.native.chunks_exact(16) {
            let x = f64::from_le_bytes(pair[..8].try_into().unwrap());
            let y = f64::from_le_bytes(pair[8..].try_into().unwrap());
            use std::fmt::Write as _;
            writeln!(&mut csv, "{x},{y}").map_err(error)?;
        }
        write_new(&directory.join("manifest.json"), &self.manifest_bytes)?;
        write_new(&directory.join("spectrum.csv"), csv.as_bytes())?;
        let seal = json!({"schema":1,"kind":"native_csv_export","source_native_sha256":self.manifest.descriptor["sha256"],
            "manifest_bytes":self.manifest_bytes.len(),"manifest_sha256":sha256_bytes(&self.manifest_bytes)?,
            "csv_bytes":csv.len(),"csv_sha256":sha256_bytes(csv.as_bytes())?});
        publish_seal(&directory, &serde_json::to_vec(&seal).map_err(error)?)?;
        Ok(directory)
    }
}
fn validate_origin(origin: &CaptureOrigin, host_id: &str) -> Result<(), HostError> {
    origin.domain.validate()?;
    if origin.host_id != host_id
        || !valid_id(&origin.operation_id)
        || !(1..=MAX_SEQUENCE).contains(&origin.config_rev)
        || !origin.device_identity.is_object()
        || origin.device_identity.as_object().unwrap().is_empty()
        || serde_json::to_vec(&origin.device_identity)
            .map_err(error)?
            .len()
            > 4096
    {
        return Err(error("Invalid capture origin"));
    }
    Ok(())
}

pub struct ArchiveStore {
    root: PathBuf,
    host_id: String,
}
impl ArchiveStore {
    pub fn open(root: &Path, host_id: &str) -> Result<Self, HostError> {
        if !valid_id(host_id) {
            return Err(error("Invalid owning Host"));
        }
        if !root.is_absolute()
            || root.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(error("Absolute normalized archive directory required"));
        }
        // Creation is one component at a time under pinned, ordinary parents.
        let mut ancestors: Vec<_> = root.ancestors().collect();
        ancestors.reverse();
        let mut guards = Vec::new();
        for path in ancestors {
            if !path.exists() {
                fs::create_dir(path).map_err(error)?;
            }
            guards.push(open_guarded(path, true)?);
        }
        let _pins = pin_directories(root)?;
        let store = Self {
            root: fs::canonicalize(root).map_err(error)?,
            host_id: host_id.to_owned(),
        };
        store.directories()?;
        Ok(store)
    }
    fn directory(&self, name: &str, id: &str) -> Result<PathBuf, HostError> {
        if !valid_name(name) || !valid_id(id) {
            return Err(error("Invalid archive name or ID"));
        }
        Ok(self.root.join(name).join(id))
    }
    fn directories(&self) -> Result<Vec<(String, String, PathBuf)>, HostError> {
        let _pins = pin_directories(&self.root)?;
        let mut records = Vec::new();
        let mut seen = 0;
        for group in fs::read_dir(&self.root).map_err(error)? {
            seen += 1;
            if seen > MAX_DIRECTORY_ENTRIES {
                return Err(error("Archive directory capacity exceeded"));
            }
            let group = group.map_err(error)?;
            let Some(name) = group.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !valid_name(&name) || !group.file_type().map_err(error)?.is_dir() {
                continue;
            }
            let _group = pin_directories(&group.path())?;
            if !group.path().join(".archive.json").exists() {
                continue;
            }
            if !self.owns_group(&group.path())? {
                continue;
            }
            for entry in fs::read_dir(group.path()).map_err(error)? {
                seen += 1;
                if seen > MAX_DIRECTORY_ENTRIES {
                    return Err(error("Archive directory capacity exceeded"));
                }
                let entry = entry.map_err(error)?;
                let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !valid_id(&id) {
                    continue;
                }
                // Reparse records are errors, not silently accepted or traversed.
                let _record = pin_directories(&entry.path())?;
                records.push((name.clone(), id, entry.path()));
                if records.len() > MAX_RECORDINGS {
                    return Err(error("Archive recording capacity exceeded"));
                }
            }
        }
        records.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        Ok(records)
    }
    fn owns_group(&self, directory: &Path) -> Result<bool, HostError> {
        let marker: Group = serde_json::from_value(
            strict_json(&bounded_file(&directory.join(".archive.json"), 1024)?).map_err(error)?,
        )
        .map_err(error)?;
        if marker.schema != 1 || !valid_id(&marker.host_id) {
            return Err(error("Invalid archive namespace marker"));
        }
        Ok(marker.host_id == self.host_id)
    }
    fn load(&self, name: &str, id: &str) -> Result<(Manifest, Vec<u8>, Vec<u8>), HostError> {
        let directory = self.directory(name, id)?;
        let _pins = pin_directories(&directory)?;
        if !fs::canonicalize(&directory)
            .map_err(error)?
            .starts_with(&self.root)
        {
            return Err(error("Archive path escaped root"));
        }
        let seal: Seal = serde_json::from_value(
            strict_json(&bounded_file(&directory.join("complete.json"), 1024)?).map_err(error)?,
        )
        .map_err(error)?;
        let manifest_bytes = bounded_file(&directory.join("manifest.json"), MAX_MANIFEST)?;
        let native = bounded_file(&directory.join("native.bin"), MAX_NATIVE)?;
        if seal.schema != 1
            || seal.manifest_bytes != manifest_bytes.len() as u64
            || seal.manifest_sha256 != sha256_bytes(&manifest_bytes)?
            || seal.native_bytes != native.len() as u64
            || seal.native_sha256 != sha256_bytes(&native)?
        {
            return Err(error("Archive seal mismatch"));
        }
        let manifest: Manifest =
            serde_json::from_value(strict_json(&manifest_bytes).map_err(error)?).map_err(error)?;
        self.validate_origin(&manifest.origin)?;
        if manifest.schema != 1
            || manifest.source_kind != "real"
            || manifest.name != name
            || manifest.origin.operation_id != id
            || manifest.archived_at_unix_ms == 0
        {
            return Err(error("Invalid archive provenance"));
        }
        validate_capture_descriptor(&manifest.descriptor).map_err(error)?;
        validate_native(&native, &manifest.descriptor)?;
        if seal.native_sha256 != manifest.descriptor["sha256"].as_str().unwrap_or("") {
            return Err(error("Descriptor seal mismatch"));
        }
        Ok((manifest, manifest_bytes, native))
    }
    fn reference(manifest: &Manifest) -> ArchiveRef {
        ArchiveRef {
            id: manifest.origin.operation_id.clone(),
            name: manifest.name.clone(),
            host_id: manifest.origin.host_id.clone(),
            domain: manifest.origin.domain.clone(),
            byte_count: manifest.descriptor["byte_count"].as_u64().unwrap(),
            sample_count: manifest.descriptor["point_count"].as_u64().unwrap(),
            sha256: manifest.descriptor["sha256"].as_str().unwrap().to_string(),
            metadata: manifest.descriptor["metadata"].clone(),
        }
    }
    fn validate_origin(&self, origin: &CaptureOrigin) -> Result<(), HostError> {
        validate_origin(origin, &self.host_id)
    }
    fn attempt(&self, name: &str, id: &str) -> Result<Attempt, HostError> {
        let directory = self.directory(name, id)?;
        let _pins = pin_directories(&directory)?;
        let attempt: Attempt = serde_json::from_value(
            strict_json(&bounded_file(
                &directory.join("attempt.json"),
                MAX_MANIFEST,
            )?)
            .map_err(error)?,
        )
        .map_err(error)?;
        self.validate_origin(&attempt.origin)?;
        if attempt.schema != 1
            || attempt.source_kind != "real"
            || attempt.name != name
            || attempt.origin.operation_id != id
            || attempt.started_at_unix_ms == 0
        {
            return Err(error("Invalid capture attempt provenance"));
        }
        Ok(attempt)
    }
    fn create_record(&self, name: &str, id: &str) -> Result<(PathBuf, Vec<File>), HostError> {
        let directory = self.directory(name, id)?;
        let mut pins = pin_directories(&self.root)?;
        let group = self.root.join(name);
        if !group.exists() {
            fs::create_dir(&group).map_err(error)?;
        }
        pins.extend(pin_directories(&group)?);
        if !group.join(".archive.json").exists() {
            if fs::read_dir(&group).map_err(error)?.next().is_some() {
                return Err(error("Result name is already used by non-archive data"));
            }
            write_new(
                &group.join(".archive.json"),
                &serde_json::to_vec(&Group {
                    schema: 1,
                    host_id: self.host_id.clone(),
                })
                .map_err(error)?,
            )?;
        }
        if !self.owns_group(&group)? {
            return Err(error("Result name belongs to another Host"));
        }
        fs::create_dir(&directory).map_err(error)?;
        pins.extend(pin_directories(&directory)?);
        Ok((directory, pins))
    }
    // Persist origin before submitting the worker. An interrupted attempt has
    // no native descriptor, samples, completion proof, or reusable authority.
    pub fn begin_attempt(&mut self, name: &str, origin: &CaptureOrigin) -> Result<(), HostError> {
        self.directory(name, &origin.operation_id)?;
        self.validate_origin(origin)?;
        let records = self.directories()?;
        for (old_name, id, _) in &records {
            if id == &origin.operation_id {
                let old = self.attempt(old_name, id)?;
                return if old_name == name && old.origin == *origin {
                    Ok(())
                } else {
                    Err(error(
                        "Capture attempt conflict; original provenance preserved",
                    ))
                };
            }
        }
        if records.len() >= MAX_RECORDINGS {
            return Err(error("Archive recording capacity exceeded"));
        }
        let (directory, _pins) = self.create_record(name, &origin.operation_id)?;
        let attempt = Attempt {
            schema: 1,
            source_kind: "real".into(),
            name: name.into(),
            started_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(error)?
                .as_millis()
                .try_into()
                .map_err(error)?,
            origin: origin.clone(),
        };
        write_new(
            &directory.join("attempt.json"),
            &serde_json::to_vec(&attempt).map_err(error)?,
        )
    }
    pub fn record_failure(
        &mut self,
        name: &str,
        origin: &CaptureOrigin,
        phase: &str,
        primary_error: &str,
        hardware_read_completed: bool,
    ) -> Result<(), HostError> {
        let directory = self.directory(name, &origin.operation_id)?;
        let _pins = pin_directories(&directory)?;
        let attempt = self.attempt(name, &origin.operation_id)?;
        if attempt.origin != *origin
            || directory.join("complete.json").exists()
            || !matches!(
                phase,
                "rejected_before_call"
                    | "superseded_before_call"
                    | "completed_readback_failed"
                    | "failed_after_call_started"
                    | "timed_out_unknown"
            )
            || primary_error.is_empty()
            || hardware_read_completed && phase != "completed_readback_failed"
        {
            return Err(error("Invalid partial capture failure"));
        }
        let failure = CaptureFailure {
            schema: 1,
            phase: phase.into(),
            primary_error: primary_error.chars().take(512).collect(),
            hardware_read_completed,
        };
        write_new(
            &directory.join("failure.json"),
            &serde_json::to_vec(&failure).map_err(error)?,
        )
    }
    pub fn list(&self) -> Result<Vec<ArchiveEntry>, HostError> {
        Ok(self
            .directories()?
            .into_iter()
            .map(|(name, id, _)| self.entry(name, id))
            .collect())
    }
    fn entry(&self, name: String, id: String) -> ArchiveEntry {
        match self.load(&name, &id) {
            Ok((manifest, _, _)) => ArchiveEntry {
                name,
                id,
                state: "complete".into(),
                reference: Some(Self::reference(&manifest)),
                error: None,
            },
            Err(error) => {
                let note = self
                    .partial_error(&name, &id)
                    .unwrap_or_else(|| error.to_string())
                    .chars()
                    .take(512)
                    .collect();
                ArchiveEntry {
                    name,
                    id,
                    state: "partial".into(),
                    reference: None,
                    error: Some(note),
                }
            }
        }
    }
    fn partial_error(&self, name: &str, id: &str) -> Option<String> {
        let directory = self.directory(name, id).ok()?;
        let _pins = pin_directories(&directory).ok()?;
        self.attempt(name, id).ok()?;
        if directory.join("complete.json").exists() {
            // A damaged sealed capture is an integrity failure, not evidence
            // that its original worker attempt never completed.
            return None;
        }
        if !directory.join("failure.json").exists() {
            if directory.join("manifest.json").exists() {
                return None;
            }
            return Some("Capture attempt incomplete; hardware completion unconfirmed".into());
        }
        let failure: CaptureFailure = serde_json::from_value(
            strict_json(&bounded_file(&directory.join("failure.json"), MAX_MANIFEST).ok()?).ok()?,
        )
        .ok()?;
        if failure.schema != 1 || failure.phase.len() > 64 || failure.primary_error.len() > 2048 {
            return None;
        }
        Some(format!("{}: {}", failure.phase, failure.primary_error))
    }
    pub fn list_page(
        &self,
        domain: Option<&DomainRef>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, HostError> {
        if !(1..=4).contains(&limit) || offset > MAX_RECORDINGS {
            return Err(error("Archive page out of range"));
        }
        if let Some(domain) = domain {
            domain.validate()?;
        }
        let mut selected = Vec::new();
        for (name, id, directory) in self.directories()? {
            let _pins = pin_directories(&directory)?;
            // Filtering uses small provenance only; full payload verification is
            // limited to the requested page, not every historical recording.
            let known_origin = bounded_file(&directory.join("manifest.json"), MAX_MANIFEST)
                .ok()
                .and_then(|bytes| strict_json(&bytes).ok())
                .and_then(|v| serde_json::from_value::<Manifest>(v).ok())
                .filter(|m| {
                    m.schema == 1
                        && m.source_kind == "real"
                        && m.name == name
                        && m.origin.operation_id == id
                        && self.validate_origin(&m.origin).is_ok()
                })
                .map(|m| m.origin)
                .or_else(|| self.attempt(&name, &id).ok().map(|a| a.origin));
            if domain.is_some_and(|d| known_origin.as_ref().is_some_and(|o| &o.domain != d)) {
                continue;
            }
            selected.push((name, id));
        }
        let count = selected.len();
        let next = offset.saturating_add(limit).min(count);
        let entries = selected
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(name, id)| self.entry(name, id))
            .collect::<Vec<_>>();
        Ok(json!({"entries":entries,"next_offset":next,"has_more":next<count}))
    }
    pub fn manifest(&self, name: &str, id: &str) -> Result<Value, HostError> {
        let (_, bytes, _) = self.load(name, id)?;
        strict_json(&bytes).map_err(error)
    }
    pub fn manifest_bytes(&self, name: &str, id: &str) -> Result<Vec<u8>, HostError> {
        let (_, bytes, _) = self.load(name, id)?;
        Ok(bytes)
    }
    // Only a native UI-selected directory may reach this internal interface.
    // No Host client method accepts an export filesystem path.
    #[cfg(test)]
    fn export_native(
        &self,
        name: &str,
        id: &str,
        selected_directory: &Path,
    ) -> Result<PathBuf, HostError> {
        let (manifest, manifest_bytes, native) = self.load(name, id)?;
        let bundle = NativeExport::verify(&Self::reference(&manifest), manifest_bytes, native)?;
        bundle.write(&SelectedDirectory::open(selected_directory)?)
    }
    pub fn read(
        &self,
        name: &str,
        id: &str,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, HostError> {
        if length == 0 || length > MAX_CHUNK || offset.checked_add(length).is_none() {
            return Err(error("Invalid archive chunk range"));
        }
        let (_, _, native) = self.load(name, id)?;
        let end = offset + length;
        if end > native.len() as u64 {
            return Err(error("Archive chunk outside native data"));
        }
        Ok(native[offset as usize..end as usize].to_vec())
    }
    pub fn import<R, C>(
        &mut self,
        name: &str,
        origin: &CaptureOrigin,
        descriptor: &Value,
        mut read: R,
        cancelled: C,
    ) -> Result<ArchiveRef, HostError>
    where
        R: FnMut(u64, u64) -> Result<Vec<u8>, HostError>,
        C: Fn() -> bool,
    {
        let directory = self.directory(name, &origin.operation_id)?;
        self.validate_origin(origin)?;
        validate_capture_descriptor(descriptor).map_err(error)?;
        let records = self.directories()?;
        let mut pending = false;
        for (old_name, id, _) in &records {
            if id == &origin.operation_id {
                let _pins = pin_directories(&self.directory(old_name, id)?)?;
                if old_name == name
                    && !directory.join("complete.json").exists()
                    && !directory.join("manifest.json").exists()
                    && !directory.join("native.bin").exists()
                    && !directory.join("failure.json").exists()
                    && self.attempt(old_name, id)?.origin == *origin
                {
                    pending = true;
                    continue;
                }
                let (old, _, _) = self.load(old_name, id)?;
                if old.name == name && old.origin == *origin && old.descriptor == *descriptor {
                    return Ok(Self::reference(&old));
                }
                return Err(error(
                    "Archive operation conflict; immutable provenance preserved",
                ));
            }
        }
        if !pending && records.len() >= MAX_RECORDINGS {
            return Err(error("Archive recording capacity exceeded"));
        }
        if cancelled() {
            return Err(error("Archive import cancelled"));
        }
        let (directory, _pins) = if pending {
            (
                directory,
                pin_directories(&self.directory(name, &origin.operation_id)?)?,
            )
        } else {
            self.create_record(name, &origin.operation_id)?
        };
        let manifest = Manifest {
            schema: 1,
            source_kind: "real".into(),
            name: name.into(),
            archived_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(error)?
                .as_millis()
                .try_into()
                .map_err(error)?,
            origin: origin.clone(),
            descriptor: descriptor.clone(),
        };
        let manifest_bytes = serde_json::to_vec(&manifest).map_err(error)?;
        if manifest_bytes.len() as u64 > MAX_MANIFEST {
            return Err(error("Archive manifest exceeds limit"));
        }
        write_new(&directory.join("manifest.json"), &manifest_bytes)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("native.bin"))
            .map_err(error)?;
        let size = descriptor["byte_count"].as_u64().unwrap();
        let mut offset = 0;
        let mut native = Vec::with_capacity(size as usize);
        while offset < size {
            if cancelled() {
                return Err(error("Archive import interrupted"));
            }
            let length = MAX_CHUNK.min(size - offset);
            let bytes = read(offset, length)?;
            if bytes.len() as u64 != length {
                return Err(error("Worker chunk length mismatch"));
            }
            file.write_all(&bytes).map_err(error)?;
            native.extend_from_slice(&bytes);
            offset += length;
        }
        validate_native(&native, descriptor)?;
        file.flush().and_then(|_| file.sync_all()).map_err(error)?;
        drop(file);
        if cancelled() {
            return Err(error("Archive import interrupted before sealing"));
        }
        let seal = Seal {
            schema: 1,
            manifest_bytes: manifest_bytes.len() as u64,
            manifest_sha256: sha256_bytes(&manifest_bytes)?,
            native_bytes: size,
            native_sha256: sha256_bytes(&native)?,
        };
        publish_seal(&directory, &serde_json::to_vec(&seal).map_err(error)?)?;
        // Read through the durable representation before reporting completion.
        let (saved, _, _) = self
            .load(name, &origin.operation_id)
            .map_err(|e| error(format!("Sealed readback: {e}")))?;
        Ok(Self::reference(&saved))
    }
}

type Job = Box<dyn FnOnce(&mut ArchiveStore) + Send>;
pub struct ArchiveActor {
    jobs: std::sync::mpsc::SyncSender<Job>,
}
impl ArchiveActor {
    pub fn new(store: ArchiveStore) -> Result<Self, HostError> {
        let (jobs, rx) = std::sync::mpsc::sync_channel::<Job>(8);
        std::thread::Builder::new()
            .name("Host capture archive".into())
            .spawn(move || {
                let mut store = store;
                while let Ok(job) = rx.recv() {
                    job(&mut store);
                }
            })
            .map_err(error)?;
        Ok(Self { jobs })
    }
    pub async fn call<T: serde::de::DeserializeOwned>(
        &self,
        job: Box<dyn FnOnce(&mut ArchiveStore) -> Result<Value, HostError> + Send>,
    ) -> Result<T, HostError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.jobs
            .try_send(Box::new(move |store| {
                let _ = tx.send(job(store));
            }))
            .map_err(|_| HostError::new("ArchiveCapacity", "Archive queue full or unavailable"))?;
        serde_json::from_value(rx.await.map_err(error)??).map_err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "archive-test-{}",
                super::super::registry::new_id().unwrap()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn origin() -> CaptureOrigin {
        CaptureOrigin {
            host_id: "a".repeat(32),
            domain: super::super::leases::DomainRef {
                kind: "device".into(),
                id: "b".repeat(32),
            },
            device_identity: json!({"manufacturer":"YOKOGAWA","model":"AQ6370E","serial":"TEST-BYTES"}),
            config_rev: 3,
            operation_id: "e".repeat(32),
        }
    }
    fn descriptor(bytes: &[u8]) -> serde_json::Value {
        let context = json!({"transfer_format":"ASCII","sample_count":2,"spacing":0,"level_unit":0,
            "x_unit":0,"trace_attribute":0,"active_trace":"TRA","center_m":1.55e-6,
            "span_m":2e-9,"resolution_m":2e-11,"sweep_mode":1});
        json!({"schema":1,"kind":"osa_trace","capture_id":"c".repeat(32),"point_count":2,"byte_count":32,
            "sha256":super::super::verification::sha256_bytes(bytes).unwrap(),
            "metadata":{"native_unit":"dBm","trace":"A","identity":"YOKOGAWA,AQ6370E,TEST-BYTES,FW",
            "read_started_at":"2026-10-06T10:00:00+00:00","read_finished_at":"2026-10-06T10:00:01+00:00",
            "elapsed_s":1.,"consistency":"unproven","context_before":context,"context_after":context}})
    }
    fn payload() -> Vec<u8> {
        [1549.0_f64, -210., 1550., -69.3149081]
            .into_iter()
            .flat_map(f64::to_le_bytes)
            .collect()
    }
    fn import(
        store: &mut ArchiveStore,
        bytes: &[u8],
    ) -> Result<ArchiveRef, super::super::contracts::HostError> {
        store.import(
            "osa",
            &origin(),
            &descriptor(bytes),
            |offset, length| Ok(bytes[offset as usize..(offset + length) as usize].to_vec()),
            || false,
        )
    }
    #[test]
    fn archive_literal_bytes_survive_reopen_with_original_provenance() {
        let fixture = Fixture::new();
        let bytes = payload();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let reference = import(&mut store, &bytes).unwrap();
        assert_eq!(reference.sample_count, 2);
        assert_eq!(reference.sha256, descriptor(&bytes)["sha256"]);
        let original = store.manifest("osa", &reference.id).unwrap();
        assert_eq!(original["source_kind"], "real");
        assert_eq!(original["origin"], serde_json::to_value(origin()).unwrap());
        assert_eq!(original["descriptor"], descriptor(&bytes));
        assert!(original["archived_at_unix_ms"].as_u64().unwrap() > 0);
        drop(store);
        let store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        assert_eq!(store.list().unwrap()[0].state, "complete");
        assert_eq!(store.read("osa", &reference.id, 0, 32).unwrap(), bytes);
        assert_eq!(store.manifest("osa", &reference.id).unwrap(), original);
        let encoded = serde_json::to_string(&original).unwrap();
        for forbidden in ["boot_id", "lease_token", "ownership_nonce", "proof"] {
            assert!(!encoded.contains(forbidden));
        }
    }
    #[test]
    fn attempt_without_descriptor_survives_restart_as_scoped_partial() {
        let fixture = Fixture::new();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        store.begin_attempt("osa", &origin()).unwrap();
        drop(store);
        let store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let page = store.list_page(Some(&origin().domain), 0, 4).unwrap();
        assert_eq!(page["entries"][0]["id"], origin().operation_id);
        assert_eq!(page["entries"][0]["state"], "partial");
        assert!(page["entries"][0]["reference"].is_null());
        assert!(page["entries"][0]["error"]
            .as_str()
            .unwrap()
            .contains("unconfirmed"));
        let foreign = DomainRef {
            kind: "device".into(),
            id: "f".repeat(32),
        };
        assert!(store.list_page(Some(&foreign), 0, 4).unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty());
        let directory = fixture.0.join("osa").join(origin().operation_id);
        assert!(!directory.join("manifest.json").exists());
        assert!(!directory.join("native.bin").exists());
        let attempt: Value =
            serde_json::from_slice(&fs::read(directory.join("attempt.json")).unwrap()).unwrap();
        assert_eq!(attempt["origin"], serde_json::to_value(origin()).unwrap());
        for forbidden in [
            "boot_id",
            "lease_token",
            "proof",
            "ownership_nonce",
            "descriptor",
        ] {
            assert!(!attempt.to_string().contains(forbidden));
        }
    }
    #[test]
    fn staging_failure_retains_primary_failure_without_a_complete_reference() {
        let fixture = Fixture::new();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        store.begin_attempt("osa", &origin()).unwrap();
        store
            .record_failure(
                "osa",
                &origin(),
                "completed_readback_failed",
                "disk interrupted",
                true,
            )
            .unwrap();
        drop(store);
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let entry = &store.list().unwrap()[0];
        assert_eq!(entry.state, "partial");
        assert!(entry.reference.is_none());
        assert!(entry.error.as_ref().unwrap().contains("disk interrupted"));
        let failure: Value = serde_json::from_slice(
            &fs::read(
                fixture
                    .0
                    .join("osa")
                    .join(origin().operation_id)
                    .join("failure.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(failure["phase"], "completed_readback_failed");
        assert_eq!(failure["hardware_read_completed"], true);
        assert_eq!(failure["primary_error"], "disk interrupted");
        assert!(import(&mut store, &payload()).is_err()); // Never retry a failed attempt.
        assert!(!fixture
            .0
            .join("osa")
            .join(origin().operation_id)
            .join("complete.json")
            .exists());
    }
    #[test]
    fn matching_attempt_can_complete_once_without_rewriting_attempt_provenance() {
        let fixture = Fixture::new();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        store.begin_attempt("osa", &origin()).unwrap();
        let path = fixture
            .0
            .join("osa")
            .join(origin().operation_id)
            .join("attempt.json");
        let attempt = fs::read(&path).unwrap();
        let reference = import(&mut store, &payload()).unwrap();
        assert_eq!(reference.id, origin().operation_id);
        assert_eq!(fs::read(path).unwrap(), attempt);
        assert_eq!(store.list().unwrap()[0].state, "complete");
        assert!(store
            .record_failure("osa", &origin(), "timed_out_unknown", "late failure", false)
            .is_err());
        fs::write(
            fixture
                .0
                .join("osa")
                .join(origin().operation_id)
                .join("native.bin"),
            b"corrupt",
        )
        .unwrap();
        let damaged = &store.list().unwrap()[0];
        assert_eq!(damaged.state, "partial");
        assert!(!damaged.error.as_ref().unwrap().contains("unconfirmed"));
        assert!(damaged.error.as_ref().unwrap().contains("seal mismatch"));
    }
    #[test]
    fn archive_does_not_claim_other_result_folders_or_foreign_host_namespace() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.0.join("console")).unwrap();
        fs::create_dir(fixture.0.join("console").join("f".repeat(32))).unwrap();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(store
            .import(
                "console",
                &origin(),
                &descriptor(&payload()),
                |_, _| panic!("foreign result read"),
                || false
            )
            .is_err());
        import(&mut store, &payload()).unwrap();
        let foreign = ArchiveStore::open(&fixture.0, &"f".repeat(32)).unwrap();
        assert!(foreign.list().unwrap().is_empty());
    }
    #[test]
    fn archive_duplicate_operation_reads_no_worker_and_conflict_cannot_rewrite() {
        let fixture = Fixture::new();
        let bytes = payload();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let first = import(&mut store, &bytes).unwrap();
        let second = store
            .import(
                "osa",
                &origin(),
                &descriptor(&bytes),
                |_, _| panic!("duplicate read"),
                || false,
            )
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(store.list().unwrap().len(), 1);
        let mut conflict = descriptor(&bytes);
        conflict["metadata"]["identity"] = json!("changed");
        assert!(store
            .import(
                "osa",
                &origin(),
                &conflict,
                |_, _| panic!("conflict read"),
                || false
            )
            .is_err());
        assert!(store
            .import(
                "renamed",
                &origin(),
                &descriptor(&bytes),
                |_, _| panic!("cross-name read"),
                || false
            )
            .is_err());
        assert_eq!(store.read("osa", &first.id, 0, 32).unwrap(), bytes);
    }
    #[test]
    fn archive_invalid_payloads_never_seal_and_are_visible_partial() {
        for values in [
            [1549.0_f64, -210., 1549., -69.],
            [1549., -210., f64::NAN, -69.],
            [-1., -210., 1550., -69.],
        ] {
            let fixture = Fixture::new();
            let bytes: Vec<u8> = values.into_iter().flat_map(f64::to_le_bytes).collect();
            let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
            assert!(import(&mut store, &bytes).is_err());
            assert_eq!(store.list().unwrap()[0].state, "partial");
            assert!(!fixture
                .0
                .join("osa")
                .join(origin().operation_id)
                .join("complete.json")
                .exists());
        }
    }
    #[test]
    fn archive_truncated_hash_and_interruption_leave_no_complete_result() {
        for mode in 0..3 {
            let fixture = Fixture::new();
            let bytes = payload();
            let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
            let mut desc = descriptor(&bytes);
            if mode == 1 {
                desc["sha256"] = json!("0".repeat(64));
            }
            assert!(store
                .import(
                    "osa",
                    &origin(),
                    &desc,
                    |_, _| Ok(if mode == 0 {
                        vec![0; 31]
                    } else {
                        bytes.clone()
                    }),
                    || mode == 2
                )
                .is_err());
            assert!(store.list().unwrap().iter().all(|r| r.state == "partial"));
        }
    }
    #[test]
    fn archive_modified_manifest_or_payload_is_not_complete_after_reopen() {
        for target in ["native.bin", "manifest.json", "complete.json"] {
            let fixture = Fixture::new();
            let bytes = payload();
            let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
            let record = import(&mut store, &bytes).unwrap();
            drop(store);
            fs::write(
                fixture.0.join("osa").join(&record.id).join(target),
                b"corrupt",
            )
            .unwrap();
            let store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
            assert_eq!(store.list().unwrap()[0].state, "partial");
            assert!(store.read("osa", &record.id, 0, 32).is_err());
        }
    }
    #[test]
    fn archive_names_ids_ranges_and_native_units_are_strict() {
        let fixture = Fixture::new();
        let bytes = payload();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        for name in ["../outside", "/root", "-bad", "", "osa.", "x/y"] {
            assert!(store
                .import(
                    name,
                    &origin(),
                    &descriptor(&bytes),
                    |_, _| panic!("invalid name read"),
                    || false
                )
                .is_err());
        }
        let record = import(&mut store, &bytes).unwrap();
        for (offset, length) in [(0, 0), (0, 16385), (32, 1), (u64::MAX, 2)] {
            assert!(store.read("osa", &record.id, offset, length).is_err());
        }
        assert!(store.read("osa", "../outside", 0, 16).is_err());
        for unit in ["V", "dBm/nm"] {
            let mut desc = descriptor(&bytes);
            desc["metadata"]["native_unit"] = json!(unit);
            assert!(store
                .import(
                    "bad",
                    &origin(),
                    &desc,
                    |_, _| panic!("invalid unit read"),
                    || false
                )
                .is_err());
        }
    }
    #[test]
    fn archive_watts_preserve_zero_and_reject_negative() {
        for power in [0.0_f64, -0.001] {
            let fixture = Fixture::new();
            let bytes: Vec<u8> = [1549.0, power, 1550., 0.001]
                .into_iter()
                .flat_map(f64::to_le_bytes)
                .collect();
            let mut desc = descriptor(&bytes);
            desc["metadata"]["native_unit"] = json!("W");
            desc["metadata"]["context_before"]["level_unit"] = json!(1);
            desc["metadata"]["context_after"]["level_unit"] = json!(1);
            desc["metadata"]["context_before"]["spacing"] = json!(1);
            desc["metadata"]["context_after"]["spacing"] = json!(1);
            let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
            let result = store.import("osa", &origin(), &desc, |_, _| Ok(bytes.clone()), || false);
            assert_eq!(result.is_ok(), power >= 0.0);
            if let Ok(record) = result {
                assert_eq!(store.read("osa", &record.id, 0, 32).unwrap(), bytes);
            }
        }
    }
    #[test]
    fn archive_seal_failure_remains_partial_across_restart_and_never_retries_read() {
        let fixture = Fixture::new();
        let bytes = payload();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let directory = fixture.0.join("osa").join(origin().operation_id);
        assert!(store
            .import(
                "osa",
                &origin(),
                &descriptor(&bytes),
                |_, _| {
                    fs::create_dir(directory.join("complete.json")).unwrap();
                    Ok(bytes.clone())
                },
                || false
            )
            .is_err());
        drop(store);
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        assert_eq!(store.list().unwrap()[0].state, "partial");
        assert!(store
            .import(
                "osa",
                &origin(),
                &descriptor(&bytes),
                |_, _| panic!("interrupted import cannot repeat hardware read"),
                || false
            )
            .is_err());
    }
    #[test]
    fn archive_cancellation_after_data_write_cannot_publish_marker() {
        let fixture = Fixture::new();
        let bytes = payload();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let cancelled = std::cell::Cell::new(false);
        assert!(store
            .import(
                "osa",
                &origin(),
                &descriptor(&bytes),
                |_, _| {
                    cancelled.set(true);
                    Ok(bytes.clone())
                },
                || cancelled.get()
            )
            .is_err());
        assert_eq!(store.list().unwrap()[0].state, "partial");
    }
    #[cfg(windows)]
    #[test]
    fn archive_directory_pins_block_replacement_and_junctions_are_rejected() {
        let fixture = Fixture::new();
        let outside = Fixture::new();
        let bytes = payload();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let directory = fixture.0.join("osa").join(origin().operation_id);
        let saved = store
            .import(
                "osa",
                &origin(),
                &descriptor(&bytes),
                |_, _| {
                    assert!(fs::rename(&directory, fixture.0.join("escaped")).is_err());
                    Ok(bytes.clone())
                },
                || false,
            )
            .unwrap();
        assert_eq!(store.read("osa", &saved.id, 0, 32).unwrap(), bytes);
        let junction = fixture.0.join("linked");
        let status = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside.0)
            .output()
            .unwrap();
        assert!(status.status.success());
        assert!(ArchiveStore::open(&junction, &origin().host_id).is_err());
        assert!(store
            .import(
                "linked",
                &origin(),
                &descriptor(&bytes),
                |_, _| panic!("junction read"),
                || false
            )
            .is_err());
        // Remove only this test-owned junction, never its external target.
        fs::remove_dir(&junction).unwrap();
        assert!(outside.0.exists());
    }
    #[test]
    fn archive_actor_has_exactly_eight_queued_slots_and_reports_backpressure() {
        let fixture = Fixture::new();
        let actor =
            ArchiveActor::new(ArchiveStore::open(&fixture.0, &origin().host_id).unwrap()).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        actor
            .jobs
            .try_send(Box::new(move |_| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }))
            .unwrap();
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        for _ in 0..8 {
            actor.jobs.try_send(Box::new(|_| {})).unwrap();
        }
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(actor.call::<Value>(Box::new(|_| Ok(json!({})))));
        release_tx.send(()).unwrap();
        assert_eq!(result.unwrap_err().code, "ArchiveCapacity");
    }
    #[test]
    fn received_export_verifies_the_exact_reference_and_original_manifest_before_any_write() {
        let fixture = Fixture::new();
        let destination = Fixture::new();
        let bytes = payload();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let reference = import(&mut store, &bytes).unwrap();
        let original = store.manifest_bytes("osa", &reference.id).unwrap();
        let bundle = NativeExport::verify(&reference, original.clone(), bytes.clone()).unwrap();
        let selected = SelectedDirectory::open(&destination.0).unwrap();
        let exported = bundle.write(&selected).unwrap();
        assert_eq!(fs::read(exported.join("manifest.json")).unwrap(), original);
        assert_eq!(
            fs::read_to_string(exported.join("spectrum.csv")).unwrap(),
            "wavelength_nm,power_dBm\n1549,-210\n1550,-69.3149081\n"
        );
        assert!(exported.join("complete.json").is_file());
        for altered in [
            {
                let mut r = reference.clone();
                r.host_id = "f".repeat(32);
                r
            },
            {
                let mut r = reference.clone();
                r.domain.id = "f".repeat(32);
                r
            },
            {
                let mut r = reference.clone();
                r.sample_count = 3;
                r
            },
            {
                let mut r = reference.clone();
                r.metadata["native_unit"] = json!("W");
                r
            },
        ] {
            assert!(NativeExport::verify(&altered, original.clone(), bytes.clone()).is_err());
        }
        assert!(NativeExport::verify(&reference, original.clone(), bytes[..16].to_vec()).is_err());
        let mut corrupted = bytes.clone();
        corrupted[0] ^= 1;
        assert!(NativeExport::verify(&reference, original.clone(), corrupted).is_err());
        let mut changed: Value = serde_json::from_slice(&original).unwrap();
        changed["origin"]["config_rev"] = json!(0);
        assert!(NativeExport::verify(
            &reference,
            serde_json::to_vec(&changed).unwrap(),
            bytes.clone()
        )
        .is_err());
        assert!(NativeExport::verify(&reference, vec![b' '; 16385], bytes).is_err());
        assert_eq!(fs::read_dir(&destination.0).unwrap().count(), 1);
    }
    #[test]
    fn selected_export_folder_is_existing_pinned_and_never_creates_a_supplied_path() {
        let fixture = Fixture::new();
        let missing = fixture.0.join("missing");
        assert!(SelectedDirectory::open(&missing).is_err());
        assert!(!missing.exists());
        assert!(SelectedDirectory::open(Path::new("relative")).is_err());
        assert!(SelectedDirectory::open(&fixture.0.join("..")).is_err());
        let selected = SelectedDirectory::open(&fixture.0).unwrap();
        let other = fixture.0.with_extension("rename");
        assert!(fs::rename(&fixture.0, &other).is_err());
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        let delete_handle = || {
            OpenOptions::new()
                .access_mode(DELETE)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&fixture.0)
        };
        assert!(delete_handle().is_err());
        assert_eq!(selected.path(), fixture.0.as_path());
        drop(selected);
        // Other parallel tests may still pin our common TEMP ancestor. Probe
        // release of this exact directory without reopening its parent for rename.
        assert!(delete_handle().is_ok());
    }
    #[test]
    fn archive_pages_are_bounded_and_exports_keep_original_manifest_and_native_values() {
        let fixture = Fixture::new();
        let destination = Fixture::new();
        let bytes = payload();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let record = import(&mut store, &bytes).unwrap();
        let original =
            fs::read(fixture.0.join("osa").join(&record.id).join("manifest.json")).unwrap();
        let page = store.list_page(Some(&origin().domain), 0, 4).unwrap();
        assert_eq!(page["entries"].as_array().unwrap().len(), 1);
        assert_eq!(page["has_more"], false);
        assert!(store.list_page(None, 0, 5).is_err());
        assert_eq!(
            store
                .list_page(
                    Some(&super::super::leases::DomainRef {
                        kind: "device".into(),
                        id: "f".repeat(32)
                    }),
                    0,
                    4
                )
                .unwrap()["entries"],
            json!([])
        );
        let exported = store
            .export_native("osa", &record.id, &destination.0)
            .unwrap();
        assert_eq!(fs::read(exported.join("manifest.json")).unwrap(), original);
        let csv = fs::read_to_string(exported.join("spectrum.csv")).unwrap();
        let rows: Vec<_> = csv.lines().collect();
        assert_eq!(rows[0], "wavelength_nm,power_dBm");
        let restored: Vec<u8> = rows[1..]
            .iter()
            .flat_map(|row| {
                row.split(',')
                    .map(|cell| cell.parse::<f64>().unwrap().to_le_bytes())
            })
            .flatten()
            .collect();
        assert_eq!(restored, bytes);
        assert!(exported.join("complete.json").is_file());
        let second = store
            .export_native("osa", &record.id, &destination.0)
            .unwrap();
        assert_ne!(second, exported);
        assert_eq!(fs::read(exported.join("manifest.json")).unwrap(), original);
    }
    #[test]
    fn archive_history_error_text_cannot_overflow_control_frames() {
        let fixture = Fixture::new();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let reference = import(&mut store, &payload()).unwrap();
        let directory = fixture.0.join("osa").join(&reference.id);
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap()).unwrap();
        manifest["x".repeat(2000)] = json!(true);
        let bytes = serde_json::to_vec(&manifest).unwrap();
        fs::write(directory.join("manifest.json"), &bytes).unwrap();
        let mut seal: Value =
            serde_json::from_slice(&fs::read(directory.join("complete.json")).unwrap()).unwrap();
        seal["manifest_bytes"] = json!(bytes.len());
        seal["manifest_sha256"] = json!(sha256_bytes(&bytes).unwrap());
        fs::write(
            directory.join("complete.json"),
            serde_json::to_vec(&seal).unwrap(),
        )
        .unwrap();
        let page = store.list_page(None, 0, 4).unwrap();
        assert_eq!(page["entries"][0]["state"], "partial");
        assert!(
            page["entries"][0]["error"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 512
        );
    }
    #[test]
    fn archive_maximum_capture_is_exact_and_every_source_chunk_is_bounded() {
        let fixture = Fixture::new();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let bytes: Vec<u8> = (0..200001)
            .flat_map(|i| [1549.0 + i as f64 * 0.001, -210.0])
            .flat_map(f64::to_le_bytes)
            .collect();
        let mut desc = descriptor(&bytes);
        desc["point_count"] = json!(200001);
        desc["byte_count"] = json!(bytes.len());
        for key in ["context_before", "context_after"] {
            desc["metadata"][key]["sample_count"] = json!(200001);
        }
        let mut requests = 0;
        let reference = store
            .import(
                "osa",
                &origin(),
                &desc,
                |offset, length| {
                    requests += 1;
                    assert!((1..=16384).contains(&length));
                    Ok(bytes[offset as usize..(offset + length) as usize].to_vec())
                },
                || false,
            )
            .unwrap();
        assert_eq!(requests, 196);
        assert_eq!(reference.sample_count, 200001);
        assert!(serde_json::to_vec(&reference).unwrap().len() < 65536);
        assert_eq!(
            store
                .read("osa", &reference.id, bytes.len() as u64 - 16, 16)
                .unwrap(),
            bytes[bytes.len() - 16..]
        );
    }
    #[test]
    fn archive_imports_maximum_native_spool_and_ack_follows_durable_readback() {
        use std::time::Duration;
        use yang_drivers::osa::*;
        use yang_worker::captures::CaptureSpool;
        let fixture = Fixture::new();
        let staging = Fixture::new();
        let nonce = yang_worker::new_id().unwrap();
        let root = staging.0.join(&nonce);
        fs::create_dir(&root).unwrap();
        let mut spool = CaptureSpool::open(root.clone(), nonce.clone()).unwrap();
        let context = TraceContext::new(TraceContextParams {
            transfer_format: TransferFormat::Ascii,
            sample_count: 200001,
            spacing: 0,
            level_unit: 0,
            x_unit: 0,
            trace_attribute: 0,
            active_trace: TraceId::A,
            center_m: 1.55e-6,
            span_m: 2e-9,
            resolution_m: 2e-11,
            sweep_mode: 1,
        })
        .unwrap();
        let capture = TraceCapture::new(
            (0..200001).map(|i| 1549. + i as f64 * 0.001).collect(),
            vec![-210.; 200001],
            NativeUnit::Dbm,
            TraceId::A,
            "YOKOGAWA,AQ6370E,TEST-BYTES,FW".into(),
            ReadTiming {
                started_utc: UNIX_EPOCH + Duration::from_secs(1791280800),
                finished_utc: UNIX_EPOCH + Duration::from_secs(1791280801),
                elapsed: Duration::from_secs(1),
                decode: Duration::from_millis(1),
                io: Duration::from_millis(900),
            },
            context.clone(),
            context,
        )
        .unwrap();
        let desc = spool.stage(&capture).unwrap();
        let value = serde_json::to_value(&desc).unwrap();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        assert!(store
            .import(
                "osa",
                &origin(),
                &value,
                |_, _| Err(error("bounded transfer failure")),
                || false
            )
            .is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        assert!(spool.read_chunk(&nonce, &desc.capture_id, 0, 16).is_ok());
        let mut completed = origin();
        completed.operation_id = "d".repeat(32);
        let mut chunks = 0;
        let reference = store
            .import(
                "osa",
                &completed,
                &value,
                |offset, length| {
                    chunks += 1;
                    assert!((1..=16384).contains(&length));
                    spool
                        .read_chunk(&nonce, &desc.capture_id, offset, length as usize)
                        .map_err(error)
                },
                || false,
            )
            .unwrap();
        assert_eq!(chunks, 196);
        assert_eq!(reference.sample_count, 200001);
        assert_eq!(reference.sha256, desc.sha256);
        assert_eq!(reference.metadata, desc.metadata);
        let last = store
            .read("osa", &reference.id, desc.byte_count - 16, 16)
            .unwrap();
        assert_eq!(f64::from_le_bytes(last[..8].try_into().unwrap()), 1749.);
        assert_eq!(f64::from_le_bytes(last[8..].try_into().unwrap()), -210.);
        assert_eq!(
            store.manifest("osa", &reference.id).unwrap()["descriptor"],
            value
        );
        spool.ack(&nonce, &desc.capture_id, &desc.sha256).unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }
    #[test]
    fn archive_invalid_root_or_count_is_rejected_before_fetch_or_creation() {
        let id = format!(
            "archive-relative-{}",
            super::super::registry::new_id().unwrap()
        );
        let path = std::path::PathBuf::from(id);
        assert!(!path.exists());
        let result = ArchiveStore::open(&path, &origin().host_id);
        let created = path.exists();
        if created {
            fs::remove_dir(&path).unwrap();
        }
        assert!(result.is_err());
        assert!(!created);
        let fixture = Fixture::new();
        let mut store = ArchiveStore::open(&fixture.0, &origin().host_id).unwrap();
        let mut desc = descriptor(&payload());
        desc["point_count"] = json!(3);
        assert!(store
            .import(
                "osa",
                &origin(),
                &desc,
                |_, _| panic!("bad count read"),
                || false
            )
            .is_err());
        assert!(store.list().unwrap().is_empty());
    }
}
