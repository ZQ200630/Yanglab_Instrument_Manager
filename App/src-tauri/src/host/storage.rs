//! Separates mutable Host storage from immutable package resources.
use super::{
    archive::{open_guarded, pin_directories, valid_name},
    contracts::{valid_id, HostError, RegistrySnapshot},
    instance::WorkerRecord,
    registry::{backup_original, read_config, Registry, RegistryChange},
};
use serde::Deserialize;
use serde_json::Value;
use std::{fs, io::Read, path::{Path, PathBuf}};

fn error(message: impl std::fmt::Display) -> HostError {
    HostError::new("StorageLocation", message.to_string())
}

pub(crate) struct StorageRoots {
    pub archive: Result<PathBuf, HostError>,
    pub console: PathBuf,
}

pub(crate) fn resolve(
    registry: &mut Registry,
    package_root: &Path,
    record_dir: &Path,
) -> Result<StorageRoots, HostError> {
    let snapshot = registry.snapshot()?;
    let console = record_dir.join("console").join(&snapshot.host_id);
    // An established root is authoritative even when unavailable. Never create
    // a second history by falling back to another directory after an error.
    if let Some(root) = &snapshot.settings.data_root {
        return Ok(StorageRoots { archive: Ok(root.into()), console });
    }

    let archive = select_and_pin(registry, package_root, record_dir, snapshot);
    if archive.is_err() {
        preserve_previous_location(record_dir)?;
    }
    // Discovery errors disable data access, not management or safe shutdown.
    Ok(StorageRoots { archive, console })
}

fn select_and_pin(
    registry: &mut Registry,
    package_root: &Path,
    record_dir: &Path,
    snapshot: RegistrySnapshot,
) -> Result<PathBuf, HostError> {
    // Read the previous location before StartupRecord::intent replaces it.
    // This is only a storage hint; InstanceGuard's ownership checks still run
    // independently, before this resolver or any Worker launch.
    let mut candidates = vec![package_root.join("Result")];
    if let Some(previous) = previous_result_root(record_dir)? {
        candidates.push(previous);
    }
    let mut legacy: Option<(PathBuf, PathBuf)> = None;
    for candidate in candidates {
        if owns_legacy_data(&candidate, &snapshot.host_id)? {
            let canonical = candidate.canonicalize().map_err(error)?;
            if let Some((_, selected)) = &legacy {
                if *selected != canonical {
                    return Err(error("Saved data exists in both current and previous package locations. Set the measurement data folder explicitly; no data was moved or merged."));
                }
            } else {
                legacy = Some((candidate, canonical));
            }
        }
    }
    let archive = match legacy {
        Some((path, _)) => path,
        None => record_dir.parent().ok_or_else(|| error("Host state has no profile directory"))?.join("Result"),
    };
    let mut settings = snapshot.settings;
    settings.data_root = Some(archive.to_str()
        .ok_or_else(|| error("Measurement data folder is not a supported text path"))?.to_owned());
    // The durable location commit must precede ownership-record replacement.
    registry.commit(snapshot.registry_rev, RegistryChange::SaveSettings(settings))?;
    Ok(archive)
}

fn present(path: &Path) -> Result<bool, HostError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(error(e)),
    }
}

fn previous_result_root(record_dir: &Path) -> Result<Option<PathBuf>, HostError> {
    let preserved = record_dir.join("worker.pre-storage.json");
    let path = if present(&preserved)? { preserved } else { record_dir.join("worker.json") };
    if !present(&path)? { return Ok(None); }
    let bytes = read_config(&path, 4096).map_err(error)?;
    let record: WorkerRecord = serde_json::from_value(
        crate::runtime::strict_json(&bytes).map_err(error)?).map_err(error)?;
    if ![1, 2].contains(&record.version) || !valid_id(&record.nonce) || record.phase != "released" {
        return Err(error("Previous Worker location is unresolved; ownership recovery is still required"));
    }
    let Some(image) = record.image_path else { return Ok(None); };
    if !image.is_absolute() || image.components().any(|c| matches!(c,
        std::path::Component::ParentDir | std::path::Component::CurDir))
        || image.file_name().is_none_or(|name| name != "yang-worker.exe")
    {
        return Err(error("Previous Worker location is not a normalized packaged image path"));
    }
    Ok(Some(image.parent().ok_or_else(|| error("Previous Worker has no package directory"))?.join("Result")))
}

fn preserve_previous_location(record_dir: &Path) -> Result<(), HostError> {
    let backup = record_dir.join("worker.pre-storage.json");
    if present(&backup)? { return Ok(()); }
    let original = record_dir.join("worker.json");
    if present(&original)? {
        let bytes = read_config(&original, 4096).map_err(error)?;
        // An immutable location hint survives management-only startup replacing
        // worker.json. It grants no authority and is never a chosen data root.
        let _guard = backup_original(&backup, &bytes).map_err(error)?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveGroup { schema: u32, host_id: String }

// Only two exact Result candidates are inspected. Do not open a Workbench:
// opening it rewrites journals and clears interrupted transfer staging.
fn owns_legacy_data(root: &Path, host_id: &str) -> Result<bool, HostError> {
    if !present(root)? { return Ok(false); }
    let _pins = pin_directories(root).map_err(error)?;
    let mut owned = false;
    let mut budget = InspectionBudget { entries: 0, json_bytes: 0 };
    for entry in fs::read_dir(root).map_err(error)? {
        budget.entry()?;
        let entry = entry.map_err(error)?;
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| valid_name(name)) else { continue; };
        let kind = entry.file_type().map_err(error)?;
        if !kind.is_dir() && !kind.is_symlink() { continue; }
        let group = root.join(name);
        let _group = open_guarded(&group, true).map_err(error)?;
        let marker = group.join(".archive.json");
        if !present(&marker)? { continue; }
        let marker: ArchiveGroup = serde_json::from_value(read_json(&marker, 1024)?).map_err(error)?;
        if marker.schema != 1 || !valid_id(&marker.host_id) {
            return Err(error("Invalid legacy archive namespace marker"));
        }
        if marker.host_id == host_id {
            inspect_tree(&group, &mut budget, 0)?;
            owned = true;
        }
    }
    let workbench = root.join("workbench").join(host_id);
    if present(&workbench)? {
        let _pins = pin_directories(&workbench).map_err(error)?;
        inspect_tree(&workbench, &mut budget, 0)?;
        owned = true;
    }
    Ok(owned)
}

struct InspectionBudget { entries: usize, json_bytes: u64 }
impl InspectionBudget {
    fn entry(&mut self) -> Result<(), HostError> {
        self.entries += 1;
        if self.entries > 16384 { return Err(error("Legacy storage inspection exceeds the directory bound; choose the folder explicitly")); }
        Ok(())
    }
}

fn read_bytes(path: &Path, maximum: u64) -> Result<Vec<u8>, HostError> {
    let file = open_guarded(path, false).map_err(error)?;
    let mut bytes = Vec::new();
    file.take(maximum + 1).read_to_end(&mut bytes).map_err(error)?;
    if bytes.len() as u64 > maximum { return Err(error("Legacy storage metadata exceeds its byte bound")); }
    Ok(bytes)
}

fn read_json(path: &Path, maximum: u64) -> Result<Value, HostError> {
    crate::runtime::strict_json(&read_bytes(path, maximum)?).map_err(error)
}

// Check ordinary entries and JSON syntax without reading immutable raw sample
// arrays. Partial captures and transfer files are retained exactly as found.
fn inspect_tree(root: &Path, budget: &mut InspectionBudget, depth: usize) -> Result<(), HostError> {
    if depth > 16 { return Err(error("Legacy storage nesting exceeds the inspection bound")); }
    let _pin = open_guarded(root, true).map_err(error)?;
    for entry in fs::read_dir(root).map_err(error)? {
        budget.entry()?;
        let path = entry.map_err(error)?.path();
        let metadata = fs::symlink_metadata(&path).map_err(error)?;
        if metadata.is_dir() {
            inspect_tree(&path, budget, depth + 1)?;
        } else {
            let _file = open_guarded(&path, false).map_err(error)?;
            if path.extension().is_some_and(|extension| extension == "json") {
                budget.json_bytes = budget.json_bytes.saturating_add(metadata.len());
                if budget.json_bytes > 64 * 1024 * 1024 {
                    return Err(error("Legacy storage metadata exceeds the inspection budget; choose the folder explicitly"));
                }
                let bytes = read_bytes(&path, 16 * 1024 * 1024)?;
                match path.file_name().and_then(|name| name.to_str()) {
                    Some(name @ ("jobs.json" | "experiments.json")) =>
                        super::workbench::validate_saved_journal(name, &bytes).map_err(error)?,
                    _ => { crate::runtime::strict_json(&bytes).map_err(error)?; }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
