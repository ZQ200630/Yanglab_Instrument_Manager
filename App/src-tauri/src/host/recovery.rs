//! Explicit ownership recovery preserves cleanup uncertainty; it never releases an instrument.
use super::*;
use crate::runtime::StopReport;
use serde_json::{json, Value};
use std::io::Write;

const MAX_INCIDENT_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitEvidence {
    confirmed: bool,
    success: Option<bool>,
    code: Option<i32>,
    source: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Incident {
    version: u32,
    original: WorkerRecord,
    process_exit: ExitEvidence,
    cleanup: Value,
    resource_released: bool,
    physical_zero_verified: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authorization {
    version: u32,
    nonce: String,
    incident_sha256: String,
    confirmed: bool,
    cleanup_unknown_acknowledged: bool,
    require_readonly_verification: bool,
    resource_released: bool,
    physical_zero_verified: bool,
}
#[derive(Debug)]
pub(super) enum ProcessState {
    Absent,
    Live(String),
    Exited(String, u32),
}
fn failure(message: impl std::fmt::Display) -> HostError {
    HostError::new("OwnershipUnknown", message.to_string())
}
fn original(record: &WorkerRecord) -> Result<WorkerRecord, HostError> {
    let mut original = record.clone();
    original.phase = "identified".into();
    let created = original
        .creation_time
        .as_deref()
        .ok_or_else(|| failure("Worker creation identity is missing"))?;
    if original.version != 2
        || original.pid.is_none_or(|pid| pid == 0)
        || created.len() != 16
        || !created
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || original.assess(Some(created), true) != Ownership::Live
        || original.image_path.as_ref().is_none_or(|path| {
            path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        })
        || original
            .package_revision
            .as_ref()
            .is_none_or(|revision| revision.chars().any(char::is_control))
    {
        return Err(failure("Original worker identity is malformed"));
    }
    Ok(original)
}
fn incident_path(dir: &Path, nonce: &str) -> PathBuf {
    dir.join("worker-incidents").join(format!("{nonce}.json"))
}
fn authorization_path(dir: &Path, nonce: &str) -> PathBuf {
    dir.join("worker-incidents")
        .join(format!("{nonce}.recovery.json"))
}
fn load_incident(dir: &Path, record: &WorkerRecord) -> Result<(Incident, Vec<u8>), HostError> {
    let expected = original(record)?;
    let bytes = super::super::registry::read_config(
        &incident_path(dir, &expected.nonce),
        MAX_INCIDENT_BYTES,
    )?;
    let value = crate::runtime::strict_json(&bytes).map_err(failure)?;
    let incident: Incident = serde_json::from_value(value).map_err(failure)?;
    let exit = &incident.process_exit;
    let source_valid = match exit.source.as_str() {
        "owned_child" => exit.code.is_some() && exit.success == exit.code.map(|code| code == 0),
        "recorded_pid_absent" => exit.success.is_none() && exit.code.is_none(),
        "recorded_process_handle" => {
            exit.code.is_some() && exit.success == exit.code.map(|code| code == 0)
        }
        _ => false,
    };
    if incident.version != 1
        || incident.original != expected
        || !exit.confirmed
        || !source_valid
        || incident.resource_released
        || incident.physical_zero_verified
        || !incident.cleanup.is_object()
    {
        return Err(failure(
            "Exited-worker incident identity or evidence is invalid",
        ));
    }
    Ok((incident, bytes))
}
fn ensure_gone(record: &WorkerRecord, state: ProcessState) -> Result<ExitEvidence, HostError> {
    match state {
        ProcessState::Absent => Ok(ExitEvidence {
            confirmed: true,
            success: None,
            code: None,
            source: "recorded_pid_absent".into(),
        }),
        ProcessState::Exited(created, code) if Some(&created) == record.creation_time.as_ref() => {
            Ok(ExitEvidence {
                confirmed: true,
                success: Some(code == 0),
                code: Some(code as i32),
                source: "recorded_process_handle".into(),
            })
        }
        ProcessState::Exited(_, _) => Err(failure(
            "Recorded PID was reused; original process identity is uncertain",
        )),
        ProcessState::Live(created) if Some(&created) != record.creation_time.as_ref() => Err(
            failure("Recorded PID was reused by a live process; recovery refused"),
        ),
        ProcessState::Live(_) => Err(failure("Original worker is still alive; recovery refused")),
    }
}
fn process_state(pid: u32) -> Result<ProcessState, HostError> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(87) {
            Ok(ProcessState::Absent)
        } else {
            Err(failure(error))
        };
    }
    let result = (|| {
        let mut created: FILETIME = unsafe { std::mem::zeroed() };
        let mut exited = created;
        let mut kernel = created;
        let mut user = created;
        if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
            == 0
        {
            return Err(failure(std::io::Error::last_os_error()));
        }
        let stamp = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        let mut code = 0;
        if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
            return Err(failure(std::io::Error::last_os_error()));
        }
        Ok(if code == 259 {
            ProcessState::Live(format!("{stamp:016x}"))
        } else {
            ProcessState::Exited(format!("{stamp:016x}"), code)
        })
    })();
    unsafe { CloseHandle(handle) };
    result
}
fn immutable(path: &Path, bytes: &[u8]) -> Result<(), HostError> {
    if bytes.len() as u64 > MAX_INCIDENT_BYTES {
        return Err(failure("Recovery evidence exceeds storage bound"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| failure("Incident parent missing"))?;
    let record_dir = parent
        .parent()
        .ok_or_else(|| failure("Record directory missing"))?;
    let _ancestors = super::super::archive::pin_directories(record_dir)?;
    match fs::create_dir(parent) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(failure(error)),
    }
    let _parent = super::super::archive::open_guarded(parent, true)?;
    if fs::symlink_metadata(path).is_ok() {
        let existing = super::super::registry::read_config(path, MAX_INCIDENT_BYTES)?;
        return if existing == bytes {
            Ok(())
        } else {
            Err(failure(
                "Immutable recovery evidence conflicts with prior incident",
            ))
        };
    }
    let temporary = parent.join(format!(".recovery-{}.tmp", new_id()?));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(failure)?;
        file.write_all(bytes).map_err(failure)?;
        file.flush().map_err(failure)?;
        file.sync_all().map_err(failure)?;
        drop(file);
        super::super::archive::rename_pinned(&temporary, path, false)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
fn receipt(dir: &Path, record: &WorkerRecord) -> Value {
    json!({"recovery_authorized":true,"incident_id":record.nonce,"incident_path":incident_path(dir,&record.nonce),
        "require_readonly_verification":true,"cleanup_unknown_acknowledged":true,
        "cleanup_verified":false,"resource_released":false,"physical_zero_verified":false})
}
impl StartupRecord {
    /// The live owning Host supplies evidence from its original owned Child handle.
    pub fn exited_unverified(&mut self, report: &StopReport) -> Result<Value, HostError> {
        if !matches!(
            self.record.phase.as_str(),
            "identified" | "exited_unverified"
        ) {
            return Err(failure(
                "Worker record cannot enter exited-unverified recovery",
            ));
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ChildExit {
            confirmed: bool,
            success: bool,
            code: Option<i32>,
        }
        let exit: ChildExit = serde_json::from_value(
            report
                .process_exit
                .clone()
                .ok_or_else(|| failure("Original child exit is unconfirmed"))?,
        )
        .map_err(failure)?;
        if !exit.confirmed
            || exit.code.is_none()
            || Some(exit.success) != exit.code.map(|code| code == 0)
            || !report.cleanup.is_object()
        {
            return Err(failure(
                "Original child exit or cleanup evidence is invalid",
            ));
        }
        let incident = Incident {
            version: 1,
            original: original(&self.record)?,
            process_exit: ExitEvidence {
                confirmed: true,
                success: Some(exit.success),
                code: exit.code,
                source: "owned_child".into(),
            },
            cleanup: report.cleanup.clone(),
            resource_released: false,
            physical_zero_verified: false,
        };
        let incident = if let Some(existing) = self.existing_incident()? {
            if existing.process_exit != incident.process_exit {
                return Err(failure(
                    "Original child exit conflicts with the durable incident",
                ));
            }
            // A later Stop can observe different cleanup details. Preserve the
            // immutable first attempt rather than letting that block recovery.
            self.persist_phase("exited_unverified")?;
            existing
        } else {
            self.preserve_incident(&incident)?;
            incident
        };
        Ok(
            json!({"phase":"exited_unverified","incident_id":self.record.nonce,"incident_path":incident_path(self.path.parent().unwrap(),&self.record.nonce),
            "cleanup_verified":false,"resource_released":false,"physical_zero_verified":false,"process_exit":incident.process_exit}),
        )
    }
    fn existing_incident(&self) -> Result<Option<Incident>, HostError> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| failure("Record directory missing"))?;
        match fs::symlink_metadata(incident_path(dir, &self.record.nonce)) {
            Ok(_) => load_incident(dir, &self.record).map(|(incident, _)| Some(incident)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(failure(error)),
        }
    }
    fn preserve_incident(&mut self, incident: &Incident) -> Result<(), HostError> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| failure("Record directory missing"))?;
        immutable(
            &incident_path(dir, &self.record.nonce),
            &serde_json::to_vec(incident).map_err(failure)?,
        )?;
        self.persist_phase("exited_unverified")
    }
    fn persist_phase(&mut self, phase: &str) -> Result<(), HostError> {
        let previous = self.record.phase.clone();
        self.record.phase = phase.into();
        if let Err(error) = self.persist() {
            self.record.phase = previous;
            return Err(error);
        }
        Ok(())
    }
    fn authorize(
        &mut self,
        probe: impl FnOnce(u32) -> Result<ProcessState, HostError>,
    ) -> Result<Value, HostError> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| failure("Record directory missing"))?
            .to_path_buf();
        let (_, bytes) = load_incident(&dir, &self.record)?;
        ensure_gone(&self.record, probe(self.record.pid.unwrap())?)?;
        let authorization = Authorization {
            version: 1,
            nonce: self.record.nonce.clone(),
            incident_sha256: super::super::verification::sha256_bytes(&bytes)?,
            confirmed: true,
            cleanup_unknown_acknowledged: true,
            require_readonly_verification: true,
            resource_released: false,
            physical_zero_verified: false,
        };
        immutable(
            &authorization_path(&dir, &self.record.nonce),
            &serde_json::to_vec(&authorization).map_err(failure)?,
        )?;
        self.persist_phase("recovery_authorized")?;
        Ok(receipt(&dir, &self.record))
    }
}
impl InstanceGuard {
    /// Explicit operator acknowledgement, with the machine guard held throughout every write.
    pub fn recover_exited(record_dir: &Path, confirm: bool) -> Result<Value, HostError> {
        Self::recover_exited_matching(record_dir, confirm, None)
    }
    /// Bind administrative stop evidence to the same original worker under the guard.
    pub fn recover_exited_matching(
        record_dir: &Path,
        confirm: bool,
        expected_incident: Option<&str>,
    ) -> Result<Value, HostError> {
        if !confirm {
            return Err(HostError::new(
                "ConfirmationRequired",
                "Explicit exited-worker recovery acknowledgement required",
            ));
        }
        let guard = Self::acquire_named("Global\\YangLabInstrumentHost")?;
        recover_matching_under_guard(
            record_dir,
            &owner_root()?,
            &guard,
            expected_incident,
            process_state,
        )
    }
}
#[cfg(test)]
pub(super) fn recover_under_guard(
    record_dir: &Path,
    root: &Path,
    _guard: &InstanceGuard,
    probe: impl Fn(u32) -> Result<ProcessState, HostError>,
) -> Result<Value, HostError> {
    recover_matching_under_guard(record_dir, root, _guard, None, probe)
}
pub(super) fn recover_matching_under_guard(
    record_dir: &Path,
    root: &Path,
    _guard: &InstanceGuard,
    expected_incident: Option<&str>,
    probe: impl Fn(u32) -> Result<ProcessState, HostError>,
) -> Result<Value, HostError> {
    let _pins = super::super::archive::pin_directories(record_dir)?;
    verify_diagnostic_owner_except(root, Some(record_dir))?;
    let bytes = super::super::registry::read_config(&record_dir.join("worker.json"), 4096)?;
    let record: WorkerRecord =
        serde_json::from_value(crate::runtime::strict_json(&bytes).map_err(failure)?)
            .map_err(failure)?;
    if expected_incident.is_some_and(|expected| {
        !super::super::contracts::valid_id(expected) || record.nonce != expected
    }) {
        return Err(failure(
            "Worker incident changed after administrative stop; recovery refused",
        ));
    }
    let mut startup = StartupRecord {
        path: record_dir.join("worker.json"),
        record,
    };
    original(&startup.record)?;
    match startup.record.phase.as_str() {
        "identified" => {
            let exit = ensure_gone(&startup.record, probe(startup.record.pid.unwrap())?)?;
            if startup.existing_incident()?.is_some() {
                // The first incident may have committed before its phase write
                // failed. Keep its evidence source and cleanup exactly intact.
                startup.persist_phase("exited_unverified")?;
            } else {
                let incident = Incident {
                    version: 1,
                    original: original(&startup.record)?,
                    process_exit: exit,
                    cleanup: json!({"steps":[{"role":"worker","action":"recovery","ok":false,"error":"No cleanup receipt was available from the prior Host"}],
                        "unreleased":["unknown"],"voltage_zero":null,"resource_release_verified":false}),
                    resource_released: false,
                    physical_zero_verified: false,
                };
                startup.preserve_incident(&incident)?;
            }
        }
        "exited_unverified" => (),
        "recovery_authorized" => {
            verify_authorized_with(record_dir, &startup.record, &probe)?;
            return Ok(receipt(record_dir, &startup.record));
        }
        _ => {
            return Err(failure(
                "Only an identified or exited-unverified worker may be explicitly recovered",
            ))
        }
    }
    startup.authorize(probe)
}
pub(super) fn verify_authorized(dir: &Path, record: &WorkerRecord) -> Result<(), HostError> {
    verify_authorized_with(dir, record, process_state)
}
pub(super) fn verify_authorized_with(
    dir: &Path,
    record: &WorkerRecord,
    probe: impl FnOnce(u32) -> Result<ProcessState, HostError>,
) -> Result<(), HostError> {
    let (_, bytes) = load_incident(dir, record)?;
    let auth = super::super::registry::read_config(&authorization_path(dir, &record.nonce), 4096)?;
    let authorization: Authorization =
        serde_json::from_value(crate::runtime::strict_json(&auth).map_err(failure)?)
            .map_err(failure)?;
    if authorization.version != 1
        || authorization.nonce != record.nonce
        || authorization.incident_sha256 != super::super::verification::sha256_bytes(&bytes)?
        || !authorization.confirmed
        || !authorization.cleanup_unknown_acknowledged
        || !authorization.require_readonly_verification
        || authorization.resource_released
        || authorization.physical_zero_verified
    {
        return Err(failure("Explicit recovery acknowledgement is invalid"));
    }
    ensure_gone(record, probe(record.pid.unwrap())?)?;
    Ok(())
}
