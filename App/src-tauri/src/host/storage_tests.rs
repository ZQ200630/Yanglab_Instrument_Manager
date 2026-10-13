use super::*;
use crate::host::registry::new_id;
use std::fs;

struct Fixture {
    root: PathBuf,
    package: PathBuf,
    records: PathBuf,
    registry: Registry,
}
fn fixture_base() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().join("tmp")
}
impl Fixture {
    fn new() -> Self {
        let root = fixture_base().join(format!("sil-storage-{}", new_id().unwrap()));
        fs::create_dir(&root).unwrap();
        let package = root.join("package");
        fs::create_dir(&package).unwrap();
        let records = root.join("profile/host");
        let registry = Registry::open(&records.join("devices.json")).unwrap();
        Self { root, package, records, registry }
    }
    fn owned_archive(&self, package: &Path, host_id: &str) -> PathBuf {
        let root = package.join("Result");
        let group = root.join("saved");
        fs::create_dir_all(&group).unwrap();
        fs::write(group.join(".archive.json"),
            serde_json::to_vec(&serde_json::json!({"schema":1,"host_id":host_id})).unwrap()).unwrap();
        fs::write(group.join("preserved.bin"), b"original measurement bytes").unwrap();
        root
    }
    fn previous_worker(&self, package: &Path) -> Vec<u8> {
        let record = crate::host::instance::WorkerRecord {
            version: 2, nonce: new_id().unwrap(), phase: "released".into(),
            pid: Some(123), creation_time: Some("0000000000000001".into()),
            image_path: Some(package.join("yang-worker.exe")),
            image_sha256: Some("a".repeat(64)), package_revision: Some("fixture".into()),
        };
        let bytes = serde_json::to_vec(&record).unwrap();
        fs::write(self.records.join("worker.json"), &bytes).unwrap();
        bytes
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.root.parent() == Some(fixture_base().as_path())
            && self.root.file_name().unwrap().to_string_lossy().starts_with("sil-storage-")
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn fresh_profile_pins_writable_storage_outside_the_package() {
    let mut f = Fixture::new();
    let selected = resolve(&mut f.registry, &f.package, &f.records).unwrap();
    assert_eq!(selected.archive.unwrap(), f.root.join("profile/Result"));
    assert!(!f.package.join("Result").exists());
    let reopened = Registry::open(&f.records.join("devices.json")).unwrap();
    assert_eq!(reopened.snapshot().unwrap().settings.data_root.map(PathBuf::from),
        Some(f.root.join("profile/Result")));
}

#[test]
fn boot_console_storage_uses_host_state_even_with_a_custom_archive() {
    let mut f = Fixture::new();
    let mut snapshot = f.registry.snapshot().unwrap();
    snapshot.settings.data_root = Some(f.root.join("external").to_str().unwrap().to_owned());
    f.registry.commit(snapshot.registry_rev,
        crate::host::registry::RegistryChange::SaveSettings(snapshot.settings)).unwrap();
    let selected = resolve(&mut f.registry, &f.package, &f.records).unwrap();
    let store = crate::host::results::ResultStore::open(&selected.console, new_id().unwrap()).unwrap();
    drop(store);
    assert!(f.records.join("console").is_dir());
    assert_eq!(selected.archive.unwrap(), f.root.join("external"));
    assert!(!f.package.join("Result").exists());
}

#[test]
fn explicit_unavailable_archive_never_falls_back_or_inspects_legacy_locations() {
    let mut f = Fixture::new();
    let external = f.root.join("unavailable");
    fs::write(&external, b"not a directory").unwrap();
    let mut snapshot = f.registry.snapshot().unwrap();
    snapshot.settings.data_root = Some(external.to_str().unwrap().to_owned());
    f.registry.commit(snapshot.registry_rev,
        crate::host::registry::RegistryChange::SaveSettings(snapshot.settings)).unwrap();
    fs::write(f.records.join("worker.json"), b"invalid legacy hint").unwrap();
    let before = fs::read(f.records.join("devices.json")).unwrap();
    let selected = resolve(&mut f.registry, &f.package, &f.records).unwrap();
    let archive = selected.archive.unwrap();
    assert_eq!(archive, external);
    assert!(crate::host::archive::ArchiveStore::open(&archive,
        &f.registry.snapshot().unwrap().host_id).is_err());
    assert_eq!(fs::read(f.records.join("devices.json")).unwrap(), before);
    assert!(!f.root.join("profile/Result").exists());
}

#[test]
fn owned_current_legacy_archive_is_pinned_and_survives_package_relocation() {
    let mut f = Fixture::new();
    let host_id = f.registry.snapshot().unwrap().host_id;
    let legacy = f.owned_archive(&f.package, &host_id);
    let selected = resolve(&mut f.registry, &f.package, &f.records).unwrap();
    assert_eq!(selected.archive.unwrap(), legacy);
    let mut reopened = Registry::open(&f.records.join("devices.json")).unwrap();
    assert_eq!(reopened.snapshot().unwrap().settings.data_root,
        Some(legacy.to_str().unwrap().to_owned()));
    assert_eq!(resolve(&mut reopened, &f.root.join("new-package"), &f.records).unwrap().archive.unwrap(),
        legacy);
    assert_eq!(fs::read(legacy.join("saved/preserved.bin")).unwrap(), b"original measurement bytes");
}

#[test]
fn exact_previous_worker_location_is_preserved_before_ownership_record_replacement() {
    let mut f = Fixture::new();
    let previous = f.root.join("previous-package");
    let host_id = f.registry.snapshot().unwrap().host_id;
    let legacy = f.owned_archive(&previous, &host_id);
    let ownership = f.previous_worker(&previous);
    let selected = resolve(&mut f.registry, &f.package, &f.records).unwrap();
    assert_eq!(selected.archive.unwrap(), legacy);
    assert_eq!(fs::read(f.records.join("worker.json")).unwrap(), ownership);
    assert_eq!(Registry::open(&f.records.join("devices.json")).unwrap()
        .snapshot().unwrap().settings.data_root, Some(legacy.to_str().unwrap().to_owned()));
}

#[test]
fn two_owned_legacy_locations_are_ambiguous_and_do_not_change_settings() {
    let mut f = Fixture::new();
    let host_id = f.registry.snapshot().unwrap().host_id;
    f.owned_archive(&f.package, &host_id);
    let previous = f.root.join("previous-package");
    f.owned_archive(&previous, &host_id);
    let ownership = f.previous_worker(&previous);
    let before = fs::read(f.records.join("devices.json")).unwrap();
    let error = resolve(&mut f.registry, &f.package, &f.records).unwrap().archive.err().unwrap();
    assert_eq!(error.code, "StorageLocation");
    assert_eq!(fs::read(f.records.join("devices.json")).unwrap(), before);
    assert_eq!(fs::read(f.records.join("worker.json")).unwrap(), ownership);
}

#[test]
fn foreign_legacy_namespace_is_left_in_place_without_adoption() {
    let mut f = Fixture::new();
    let legacy = f.owned_archive(&f.package, &"f".repeat(32));
    let selected = resolve(&mut f.registry, &f.package, &f.records).unwrap();
    assert_eq!(selected.archive.unwrap(), f.root.join("profile/Result"));
    assert_eq!(fs::read(legacy.join("saved/preserved.bin")).unwrap(), b"original measurement bytes");
}

#[test]
fn malformed_legacy_namespace_is_not_adopted_or_hidden_by_a_fresh_default() {
    let mut f = Fixture::new();
    let host_id = f.registry.snapshot().unwrap().host_id;
    let legacy = f.owned_archive(&f.package, &host_id);
    fs::write(legacy.join("saved/.archive.json"), b"not JSON").unwrap();
    assert_eq!(resolve(&mut f.registry, &f.package, &f.records).unwrap().archive.err().unwrap().code,
        "StorageLocation");
    assert_eq!(f.registry.snapshot().unwrap().settings.data_root, None);
}

#[test]
fn owned_workbench_namespace_is_preserved_without_opening_or_rewriting_it() {
    let mut f = Fixture::new();
    let host_id = f.registry.snapshot().unwrap().host_id;
    let workbench = f.package.join("Result/workbench").join(&host_id);
    fs::create_dir_all(workbench.join("records")).unwrap();
    fs::write(workbench.join("jobs.json"), b"[ ]").unwrap();
    let selected = resolve(&mut f.registry, &f.package, &f.records).unwrap();
    assert_eq!(selected.archive.unwrap(), f.package.join("Result"));
    assert_eq!(fs::read(workbench.join("jobs.json")).unwrap(), b"[ ]");
}

#[test]
fn malformed_workbench_metadata_is_not_adopted() {
    let mut f = Fixture::new();
    let host_id = f.registry.snapshot().unwrap().host_id;
    let workbench = f.package.join("Result/workbench").join(&host_id);
    fs::create_dir_all(&workbench).unwrap();
    fs::write(workbench.join("jobs.json"), b"not JSON").unwrap();
    assert_eq!(resolve(&mut f.registry, &f.package, &f.records).unwrap().archive.err().unwrap().code,
        "StorageLocation");
    assert_eq!(f.registry.snapshot().unwrap().settings.data_root, None);
}

fn rejects_journal_without_committing_or_cleaning(name: &str, bytes: &[u8]) {
    let mut f = Fixture::new();
    let host_id = f.registry.snapshot().unwrap().host_id;
    f.owned_archive(&f.package, &host_id);
    let workbench = f.package.join("Result/workbench").join(&host_id);
    fs::create_dir_all(workbench.join("imports")).unwrap();
    fs::write(workbench.join("jobs.json"), b"[]").unwrap();
    fs::write(workbench.join(name), bytes).unwrap();
    let staging = workbench.join("imports").join(format!("{}.csv", "a".repeat(32)));
    fs::write(&staging, b"unfinished original transfer").unwrap();
    let before = fs::read(f.records.join("devices.json")).unwrap();
    let selected = resolve(&mut f.registry, &f.package, &f.records).unwrap();
    assert!(selected.archive.is_err(), "Malformed typed journal must disable adoption");
    assert_eq!(selected.archive.err().unwrap().code, "StorageLocation");
    assert_eq!(fs::read(f.records.join("devices.json")).unwrap(), before);
    assert_eq!(fs::read(workbench.join(name)).unwrap(), bytes);
    assert_eq!(fs::read(staging).unwrap(), b"unfinished original transfer");
    assert!(!f.root.join("profile/Result").exists());
    let mut snapshot = f.registry.snapshot().unwrap();
    assert_eq!(snapshot.settings.data_root, None);
    snapshot.settings.data_root = Some(f.root.join("operator-choice").to_str().unwrap().into());
    f.registry.commit(snapshot.registry_rev,
        crate::host::registry::RegistryChange::SaveSettings(snapshot.settings)).unwrap();
}

#[test]
fn typed_jobs_missing_fields_do_not_pin_or_clean_legacy_storage() {
    rejects_journal_without_committing_or_cleaning("jobs.json", b"[{}]");
}

#[test]
fn typed_experiments_missing_fields_do_not_pin_or_clean_legacy_storage() {
    rejects_journal_without_committing_or_cleaning("experiments.json", b"[{}]");
}

fn saved_job(id: &str) -> serde_json::Value {
    serde_json::json!({"id":id,"owner":"fixture","kind":"analysis","status":"completed",
        "stage":"completed","completed_units":1,"total_units":1,"started_at_unix_ms":1,
        "finished_at_unix_ms":2,"result":null,"error":null})
}

#[test]
fn typed_jobs_invalid_identity_is_not_adopted() {
    rejects_journal_without_committing_or_cleaning("jobs.json",
        &serde_json::to_vec(&vec![saved_job("invalid")]).unwrap());
}

#[test]
fn typed_jobs_above_existing_count_bound_are_not_adopted() {
    rejects_journal_without_committing_or_cleaning("jobs.json",
        &serde_json::to_vec(&vec![saved_job(&"a".repeat(32)); 129]).unwrap());
}

#[test]
fn typed_experiments_above_existing_byte_bound_are_not_adopted() {
    let mut bytes = vec![b' '; 2 * 1024 * 1024 + 1];
    bytes[..2].copy_from_slice(b"[]");
    rejects_journal_without_committing_or_cleaning("experiments.json", &bytes);
}

#[test]
fn typed_experiments_use_the_native_definition_validator_before_adoption() {
    let definition = serde_json::json!({"schema":2,"id":"a".repeat(32),"name":"fixture",
        "template_id":"waveform_capture","template_version":1,"roles":[],"parameters":{}});
    rejects_journal_without_committing_or_cleaning("experiments.json",
        &serde_json::to_vec(&vec![definition]).unwrap());
}

#[test]
fn unresolved_storage_leaves_management_settings_writable_without_an_alternate_root() {
    let mut f = Fixture::new();
    let host_id = f.registry.snapshot().unwrap().host_id;
    let legacy = f.owned_archive(&f.package, &host_id);
    fs::write(legacy.join("saved/.archive.json"), b"not JSON").unwrap();
    let result = resolve(&mut f.registry, &f.package, &f.records);
    assert!(result.is_ok(), "Storage discovery must not prevent Host management startup");
    assert_eq!(result.unwrap().archive.err().unwrap().code, "StorageLocation");
    let mut snapshot = f.registry.snapshot().unwrap();
    assert_eq!(snapshot.settings.data_root, None);
    assert!(!f.root.join("profile/Result").exists());
    snapshot.settings.data_root = Some(f.root.join("chosen").to_str().unwrap().to_owned());
    f.registry.commit(snapshot.registry_rev,
        crate::host::registry::RegistryChange::SaveSettings(snapshot.settings)).unwrap();
}

#[test]
fn unresolved_previous_location_hint_survives_a_management_host_restart() {
    let mut f = Fixture::new();
    let host_id = f.registry.snapshot().unwrap().host_id;
    let previous = f.root.join("previous-package");
    let legacy = f.owned_archive(&previous, &host_id);
    fs::write(legacy.join("saved/.archive.json"), b"not JSON").unwrap();
    let previous_bytes = f.previous_worker(&previous);
    assert!(resolve(&mut f.registry, &f.package, &f.records).is_ok(),
        "Storage errors must leave the management Host reachable");
    assert_eq!(fs::read(f.records.join("worker.pre-storage.json")).unwrap(), previous_bytes);
    f.previous_worker(&f.package);
    fs::write(legacy.join("saved/.archive.json"),
        serde_json::to_vec(&serde_json::json!({"schema":1,"host_id":host_id})).unwrap()).unwrap();
    assert_eq!(resolve(&mut f.registry, &f.package, &f.records).unwrap().archive.unwrap(), legacy);
}

#[cfg(windows)]
#[test]
fn reparse_legacy_root_is_never_followed_or_adopted() {
    let mut f = Fixture::new();
    let outside = f.root.join("elsewhere");
    let host_id = f.registry.snapshot().unwrap().host_id;
    let legacy = f.owned_archive(&outside, &host_id);
    let link = f.package.join("Result");
    let status = std::process::Command::new("cmd.exe")
        .args(["/C", "mklink", "/J"]).arg(&link).arg(&legacy).status().unwrap();
    assert!(status.success());
    let result = resolve(&mut f.registry, &f.package, &f.records);
    fs::remove_dir(&link).unwrap();
    assert_eq!(result.unwrap().archive.err().unwrap().code, "StorageLocation");
    assert_eq!(f.registry.snapshot().unwrap().settings.data_root, None);
}
