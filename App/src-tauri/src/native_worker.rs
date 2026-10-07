//! Fixed native package launch; never searches interpreters or PATH.
use crate::runtime::RuntimeError;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use yang_protocol::PackageIdentity;
#[derive(Clone, Debug)]
pub struct NativeWorkerLaunch {
    executable: PathBuf,
    package: PackageIdentity,
    _pins: Arc<Vec<std::fs::File>>,
    #[cfg(test)]
    test_arguments: Vec<std::ffi::OsString>,
    #[cfg(test)]
    _test_folder: Option<Arc<TestFolder>>,
}
impl NativeWorkerLaunch {
    pub fn resolve(resources: &Path, package: &PackageIdentity) -> Result<Self, RuntimeError> {
        if package.schema != 1
            || package.protocol_version != 3
            || package.startup_revision != 1
            || [
                package.source_revision.as_str(),
                package.package_revision.as_str(),
            ]
            .iter()
            .any(|s| s.is_empty() || s.len() > 128 || s.chars().any(|c| c < ' '))
            || package.worker_sha256.len() != 64
            || !package
                .worker_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(error("Native package contract mismatch"));
        }
        if !resources.is_absolute()
            || resources
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(error(
                "An absolute fixed native package directory is required",
            ));
        }
        let mut pins = Vec::new();
        for parent in resources.ancestors().collect::<Vec<_>>().into_iter().rev() {
            pins.push(pin(parent, true)?);
        }
        let executable = resources.join("yang-worker.exe");
        let mut image = pin(&executable, false)?;
        let length = image.metadata().map_err(error)?.len();
        if length == 0 || length > 128 * 1024 * 1024 {
            return Err(error("Native image size is invalid"));
        }
        let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = [0u8; 65536];
        loop {
            let n = image.read(&mut buffer).map_err(error)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        let actual: String = hash
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if actual != package.worker_sha256 {
            return Err(error("Native image SHA256 differs from this package"));
        }
        pins.push(image);
        Ok(Self {
            executable: executable.canonicalize().map_err(error)?,
            package: package.clone(),
            _pins: Arc::new(pins),
            #[cfg(test)]
            test_arguments: vec![],
            #[cfg(test)]
            _test_folder: None,
        })
    }
    pub fn packaged(resources: &Path) -> Result<Self, RuntimeError> {
        let mut file = pin(&resources.join("native-package.json"), false)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(16385)
            .read_to_end(&mut bytes)
            .map_err(error)?;
        if bytes.len() > 16384 {
            return Err(error("Native package manifest exceeds 16 KiB"));
        }
        let value = crate::runtime::strict_json(&bytes).map_err(error)?;
        let package: PackageIdentity = serde_json::from_value(value).map_err(error)?;
        let mut launch = Self::resolve(resources, &package)?;
        Arc::get_mut(&mut launch._pins).unwrap().push(file);
        Ok(launch)
    }
    pub fn executable(&self) -> &Path {
        &self.executable
    }
    pub fn package(&self) -> &PackageIdentity {
        &self.package
    }
    pub(crate) fn command(&self) -> std::process::Command {
        let command = std::process::Command::new(&self.executable);
        #[cfg(test)]
        {
            let mut command = command;
            command.args(&self.test_arguments);
            command
        }
        #[cfg(not(test))]
        command
    }
    #[cfg(test)]
    pub(crate) fn test_fixture(root: &Path) -> Result<Self, RuntimeError> {
        Self::test_image("worker-fixture.exe", Some(root))
    }
    #[cfg(test)]
    pub(crate) fn test_osa_fixture(root: &Path) -> Result<Self, RuntimeError> {
        let mut launch = Self::test_fixture(root)?;
        launch.test_arguments[1] = "osa".into();
        Ok(launch)
    }
    #[cfg(test)]
    pub(crate) fn test_native() -> Result<Self, RuntimeError> {
        Self::test_image("yang-worker.exe", None)
    }
    #[cfg(test)]
    fn test_image(name: &str, marker_root: Option<&Path>) -> Result<Self, RuntimeError> {
        let current = std::env::current_exe().map_err(error)?;
        let source = current
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| error("test binary directory missing"))?
            .join(name);
        let folder = Arc::new(TestFolder(std::env::temp_dir().join(format!(
            "yang-native-test-{}",
            crate::host::registry::new_id().map_err(error)?
        ))));
        std::fs::create_dir(&folder.0).map_err(error)?;
        std::fs::copy(source, folder.0.join("yang-worker.exe")).map_err(error)?;
        let bytes = std::fs::read(folder.0.join("yang-worker.exe")).map_err(error)?;
        let package = PackageIdentity {
            schema: 1,
            source_revision: "test-source".into(),
            package_revision: "0.1.0".into(),
            protocol_version: 3,
            startup_revision: 1,
            worker_sha256: crate::host::verification::sha256_bytes(&bytes).map_err(error)?,
        };
        let mut launch = Self::resolve(&folder.0, &package)?;
        if let Some(root) = marker_root {
            launch.test_arguments = vec![
                "--fixture-profile".into(),
                "pipe".into(),
                "--fixture-root".into(),
                root.join("App/worker").into_os_string(),
            ];
        }
        launch._test_folder = Some(folder);
        Ok(launch)
    }
}
#[cfg(test)]
#[derive(Debug)]
struct TestFolder(PathBuf);
#[cfg(test)]
impl Drop for TestFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn error(message: impl std::fmt::Display) -> RuntimeError {
    RuntimeError {
        message: message.to_string(),
        retained_runtime: None,
    }
}
#[cfg(windows)]
fn pin(path: &Path, directory: bool) -> Result<std::fs::File, RuntimeError> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::*;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(error)?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: owned open file handle and a correctly sized writable ABI structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return Err(error(std::io::Error::last_os_error()));
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        || (!directory && info.nNumberOfLinks != 1)
    {
        return Err(error(
            "Native package path is a reparse point, alias or wrong file type",
        ));
    }
    Ok(file)
}
#[cfg(not(windows))]
fn pin(_path: &Path, _directory: bool) -> Result<std::fs::File, RuntimeError> {
    Err(error("Native Windows package required"))
}
