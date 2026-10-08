use super::{
    contracts::HostError,
    registry::{new_id, write_atomic},
};
use crate::runtime::ChildIdentity;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::FILETIME,
    Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE},
    System::Threading::{
        CreateMutexW, GetExitCodeProcess, GetProcessTimes, OpenProcess,
        PROCESS_QUERY_LIMITED_INFORMATION,
    },
};

pub struct InstanceGuard {
    handle: isize,
}
impl InstanceGuard {
    pub fn acquire(record_dir: &Path) -> Result<Self, HostError> {
        let guard = Self::acquire_named("Global\\YangLabInstrumentHost")?;
        let root = owner_root()?;
        verify_diagnostic_owner(&root)?;
        verify_record(record_dir)?;
        // Publish the machine-owner record location before any child can launch.
        // Profiles and explicitly chosen Host record directories share this index.
        write_atomic(
            &root.join("owner-location.json"),
            &serde_json::to_vec(&serde_json::json!({"version":1,"record_dir":record_dir}))
                .map_err(|e| HostError::new("OwnershipStorage", e.to_string()))?,
        )?;
        Ok(guard)
    }
    pub fn acquire_diagnostic() -> Result<Self, HostError> {
        let guard = Self::acquire_named("Global\\YangLabInstrumentHost")?;
        verify_diagnostic_owner(&owner_root()?)?;
        Ok(guard)
    }
    pub(crate) fn acquire_named(name: &str) -> Result<Self, HostError> {
        let security = super::ipc::PipeSecurity::current()?;
        let descriptor =
            super::ipc::SecurityDescriptor::new(&format!("D:P(A;;0x001f0001;;;{})", security.sid))?;
        let attributes = descriptor.attributes();
        let wide = super::ipc::wide(name)?;
        // Owned live descriptor and UTF-16 name; the guard owns the handle.
        let handle = unsafe { CreateMutexW(&attributes, 0, wide.as_ptr()) };
        if handle.is_null() {
            return Err(HostError::new(
                "HostAlreadyRunning",
                format!(
                    "Machine ownership unavailable: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(handle) };
            return Err(HostError::new(
                "HostAlreadyRunning",
                "Another Host owns this machine namespace",
            ));
        }
        Ok(Self {
            handle: handle as isize,
        })
    }
}
fn owner_root() -> Result<PathBuf, HostError> {
    let root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| HostError::new("OwnershipUnknown", "Windows AppData unavailable"))?;
    Ok(root.join("edu.wustl.yanglab.silconsole"))
}
/// Read-only orphan reconciliation, also shared by standalone diagnostics.
pub fn verify_diagnostic_owner(root: &Path) -> Result<(), HostError> {
    let index = root.join("owner-location.json");
    if fs::symlink_metadata(&index).is_ok() {
        let bytes = super::registry::read_config(&index, 8192)
            .map_err(|e| HostError::new("OwnershipUnknown", e.to_string()))?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Location {
            version: u32,
            record_dir: PathBuf,
        }
        let location: Location = serde_json::from_value(
            crate::runtime::strict_json(&bytes)
                .map_err(|e| HostError::new("OwnershipUnknown", e))?,
        )
        .map_err(|e| HostError::new("OwnershipUnknown", e.to_string()))?;
        if location.version != 1 || !location.record_dir.is_absolute() {
            return Err(HostError::new(
                "OwnershipUnknown",
                "Invalid owner record location",
            ));
        }
        if !location.record_dir.join("worker.json").exists() {
            return Err(HostError::new(
                "OwnershipUnknown",
                "Indexed worker record is missing; no replacement admitted",
            ));
        }
        verify_record(&location.record_dir)?;
    }
    // Backward-compatible reconciliation for installations predating the index.
    verify_record(&root.join("host"))?;
    let profiles = root.join("profiles");
    if profiles.exists() {
        let _pins = super::archive::pin_directories(&profiles)?;
        for (n, entry) in fs::read_dir(profiles)
            .map_err(|e| HostError::new("OwnershipUnknown", e.to_string()))?
            .enumerate()
        {
            if n >= 64 {
                return Err(HostError::new("OwnershipUnknown", "Too many Host profiles"));
            }
            let path = entry
                .map_err(|e| HostError::new("OwnershipUnknown", e.to_string()))?
                .path();
            let _pin = super::archive::open_guarded(&path, true)?;
            verify_record(&path.join("host"))?;
        }
    }
    Ok(())
}
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle as HANDLE) };
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRecord {
    pub version: u64,
    pub nonce: String,
    pub phase: String,
    pub pid: Option<u32>,
    pub creation_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_revision: Option<String>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Ownership {
    Live,
    Released,
    Unknown,
}
impl WorkerRecord {
    pub fn assess(&self, creation: Option<&str>, nonce_verified: bool) -> Ownership {
        if ![1, 2].contains(&self.version) || !super::contracts::valid_id(&self.nonce) {
            return Ownership::Unknown;
        }
        if self.version == 2
            && self.phase == "identified"
            && (!self.image_path.as_ref().is_some_and(|p| p.is_absolute())
                || !self.image_sha256.as_ref().is_some_and(|s| {
                    s.len() == 64
                        && s.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
                || !self
                    .package_revision
                    .as_ref()
                    .is_some_and(|s| !s.is_empty() && s.len() <= 128))
        {
            return Ownership::Unknown;
        }
        if self.phase == "released" {
            return Ownership::Released;
        }
        if self.phase == "identified"
            && self.pid.is_some()
            && creation.is_some()
            && creation == self.creation_time.as_deref()
            && nonce_verified
        {
            return Ownership::Live;
        }
        Ownership::Unknown
    }
}
pub(crate) fn verify_record(dir: &Path) -> Result<(), HostError> {
    let path = dir.join("worker.json");
    match fs::symlink_metadata(&path) {
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(HostError::new("OwnershipUnknown", error.to_string())),
    };
    let bytes = super::registry::read_config(&path, 4096)
        .map_err(|e| HostError::new("OwnershipUnknown", e.to_string()))?;
    if bytes.len() > 4096 {
        return Err(HostError::new(
            "OwnershipUnknown",
            "Oversized startup record",
        ));
    }
    let value = crate::runtime::strict_json(&bytes)
        .map_err(|error| HostError::new("OwnershipUnknown", error))?;
    let record: WorkerRecord = serde_json::from_value(value)
        .map_err(|error| HostError::new("OwnershipUnknown", error.to_string()))?;
    // A recorded nonce is not a live nonce challenge. Unresolved intents/children
    // require explicit recovery, even if their PID no longer exists.
    if record.assess(None, false) != Ownership::Released {
        let creation = record.pid.map(process_creation).transpose()?.flatten();
        let matched = creation.is_some() && creation.as_deref() == record.creation_time.as_deref();
        return Err(HostError::new("OwnershipUnknown",format!("Unresolved worker ownership; live PID/creation match={matched}, nonce cannot be independently verified; no new worker was spawned")));
    }
    Ok(())
}
fn process_creation(pid: u32) -> Result<Option<String>, HostError> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(87) {
            Ok(None)
        } else {
            Err(HostError::new("OwnershipUnknown", error.to_string()))
        };
    }
    let result = (|| {
        let mut exit_code = 0;
        if unsafe { GetExitCodeProcess(handle, &mut exit_code) } == 0 {
            return Err(HostError::new(
                "OwnershipUnknown",
                std::io::Error::last_os_error().to_string(),
            ));
        }
        if exit_code != 259 {
            return Ok(None);
        }
        let mut created: FILETIME = unsafe { std::mem::zeroed() };
        let mut exited = created;
        let mut kernel = created;
        let mut user = created;
        if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
            == 0
        {
            return Err(HostError::new(
                "OwnershipUnknown",
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let stamp = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        Ok(Some(format!("{stamp:016x}")))
    })();
    unsafe { CloseHandle(handle) };
    result
}
pub(crate) struct StartupRecord {
    path: PathBuf,
    record: WorkerRecord,
}
impl StartupRecord {
    pub fn intent(dir: &Path) -> Result<Self, HostError> {
        let state = Self {
            path: dir.join("worker.json"),
            record: WorkerRecord {
                version: 2,
                nonce: new_id()?,
                phase: "intent".into(),
                pid: None,
                creation_time: None,
                image_path: None,
                image_sha256: None,
                package_revision: None,
            },
        };
        state.persist()?;
        Ok(state)
    }
    fn persist(&self) -> Result<(), HostError> {
        write_atomic(
            &self.path,
            &serde_json::to_vec(&self.record)
                .map_err(|error| HostError::new("OwnershipStorage", error.to_string()))?,
        )
    }
    pub fn nonce(&self) -> &str {
        &self.record.nonce
    }
    pub fn identified(&mut self, child: &ChildIdentity) -> Result<(), HostError> {
        if child.ownership_nonce != self.record.nonce {
            return Err(HostError::new(
                "OwnershipUnknown",
                "Child nonce differs from durable intent",
            ));
        }
        self.record.phase = "identified".into();
        self.record.pid = Some(child.pid);
        self.record.creation_time = Some(child.creation_time.clone());
        self.record.image_path = Some(child.image_path.clone());
        self.record.image_sha256 = Some(child.image_sha256.clone());
        self.record.package_revision = Some(child.package_revision.clone());
        self.persist()
    }
    pub fn released(&mut self) -> Result<(), HostError> {
        self.record.phase = "released".into();
        self.persist()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn second_host_fails_without_spawning_worker() {
        let tag = super::super::registry::new_id().unwrap();
        let first = InstanceGuard::acquire_named(&format!("Global\\YangLab-test-{tag}")).unwrap();
        let second = InstanceGuard::acquire_named(&format!("Global\\YangLab-test-{tag}"));
        assert_eq!(second.err().unwrap().code, "HostAlreadyRunning");
        drop(first);
    }
    #[test]
    fn pid_reuse_is_not_liveness_proof() {
        let record = WorkerRecord {
            version: 1,
            nonce: "a".repeat(32),
            phase: "identified".into(),
            pid: Some(42),
            creation_time: Some("00000000000000ff".into()),
            image_path: None,
            image_sha256: None,
            package_revision: None,
        };
        assert_eq!(
            record.assess(Some("00000000000000aa"), true),
            Ownership::Unknown
        );
        assert_eq!(
            record.assess(Some("00000000000000ff"), false),
            Ownership::Unknown
        );
        assert_eq!(
            record.assess(Some("00000000000000ff"), true),
            Ownership::Live
        );
        let mut intent = record.clone();
        intent.phase = "intent".into();
        intent.pid = None;
        assert_eq!(intent.assess(None, false), Ownership::Unknown);
    }
}
