use std::path::{Path, PathBuf};
pub fn resolve_packaged_host(
    resource_dir: &Path,
) -> Result<PathBuf, crate::host::contracts::HostError> {
    let missing =
        |message: &str| crate::host::contracts::HostError::new("HostPackageMissing", message);
    let root = resource_dir
        .canonicalize()
        .map_err(|_| missing("Host resource directory is missing"))?;
    let host = root.join("yang-lab-host.exe");
    if !host.is_file() {
        return Err(missing(
            "Bundled yang-lab-host.exe is missing; no source or Python fallback",
        ));
    }
    let canonical = host
        .canonicalize()
        .map_err(|_| missing("Host sidecar is unavailable"))?;
    if !canonical.starts_with(&root) {
        return Err(missing("Host sidecar escapes its fixed resource directory"));
    }
    Ok(canonical)
}

pub(crate) fn source_fallback_permitted(tauri_dev: bool, debug_build: bool) -> bool {
    tauri_dev && debug_build
}

pub(crate) fn resolve_worker_root(
    resources: Option<&Path>,
    source: &Path,
    allow_dev_source: bool,
) -> Result<PathBuf, String> {
    if let Some(resources) = resources {
        if resources.join("App/worker/main.py").is_file()
            && resources.join("Code/Utils/osa.py").is_file()
        {
            return resources
                .canonicalize()
                .map_err(|error| format!("resource path is unavailable: {error}"));
        }
    }
    if allow_dev_source {
        if source.join("App/worker/main.py").is_file() && source.join("Code/Utils/osa.py").is_file()
        {
            return source
                .canonicalize()
                .map_err(|error| format!("source path is unavailable: {error}"));
        }
        return Err("instrument worker package is missing from development source".to_string());
    }
    Err("instrument worker package is missing from bundled resources".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_sidecar_never_uses_source_fallback() {
        let fixture = Fixture::new();
        let source = fixture.package("source");
        let error = resolve_packaged_host(&source).unwrap_err();
        assert_eq!(error.code, "HostPackageMissing");
    }
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    fn fixture_base() -> PathBuf {
        #[cfg(target_os = "wasi")]
        {
            PathBuf::from("/tmp")
        }
        #[cfg(not(target_os = "wasi"))]
        {
            std::env::temp_dir()
        }
    }

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let root = fixture_base().join(format!(
                "sil-worker-root-{nonce}-{}",
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("create isolated fixture");
            Self { root }
        }

        fn package(&self, name: &str) -> PathBuf {
            let root = self.root.join(name);
            fs::create_dir_all(root.join("App/worker")).expect("create worker fixture");
            fs::create_dir_all(root.join("Code/Utils")).expect("create driver fixture");
            fs::write(root.join("App/worker/main.py"), "# fixture\n")
                .expect("write worker fixture");
            fs::write(root.join("Code/Utils/osa.py"), "# fixture\n").expect("write driver fixture");
            root
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.root.parent() == Some(fixture_base().as_path())
                && self
                    .root
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("sil-worker-root-"))
            {
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }

    #[test]
    fn installed_mode_rejects_missing_resources_even_when_source_exists() {
        let fixture = Fixture::new();
        let source = fixture.package("source");
        let resources = fixture.root.join("resources");
        fs::create_dir(&resources).expect("create empty resources");

        assert!(resolve_worker_root(Some(&resources), &source, false).is_err());
    }

    #[test]
    fn installed_mode_rejects_each_incomplete_bundle() {
        for missing in ["App/worker/main.py", "Code/Utils/osa.py"] {
            let fixture = Fixture::new();
            let source = fixture.package("source");
            let resources = fixture.package("resources");
            fs::remove_file(resources.join(missing)).expect("remove one bundled sentinel");

            assert!(
                resolve_worker_root(Some(&resources), &source, false).is_err(),
                "bundle missing {missing} must not be accepted"
            );
        }
    }

    #[test]
    fn installed_mode_selects_the_complete_bundled_package() {
        let fixture = Fixture::new();
        let source = fixture.package("source");
        let resources = fixture.package("resources");

        assert_eq!(
            resolve_worker_root(Some(&resources), &source, false).unwrap(),
            resources.canonicalize().unwrap()
        );
    }

    #[test]
    fn development_mode_uses_source_only_when_bundle_is_unavailable() {
        let fixture = Fixture::new();
        let source = fixture.package("source");
        let resources = fixture.root.join("resources");

        assert_eq!(
            resolve_worker_root(Some(&resources), &source, true).unwrap(),
            source.canonicalize().unwrap()
        );
    }

    #[test]
    fn source_fallback_requires_both_tauri_dev_mode_and_debug_build() {
        assert!(source_fallback_permitted(true, true));
        assert!(!source_fallback_permitted(true, false));
        assert!(!source_fallback_permitted(false, true));
        assert!(!source_fallback_permitted(false, false));
    }
}
