//! Fixed vendor package installation. No caller-supplied paths or commands.
use super::contracts::HostError;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, sync::Mutex};

#[derive(Deserialize)]
struct Package { id: String, kind: String, entry: String, files: Vec<PackageFile> }
#[derive(Deserialize)]
struct PackageFile { path: String, size: u64, sha256: String }
fn package_spec(driver: &str) -> Result<Package, HostError> {
    let packages: Vec<Package> = serde_json::from_str(include_str!("../../../drivers/packages.json"))
        .map_err(|e| HostError::new("DriverPackage", e.to_string()))?;
    packages.into_iter().find(|p|p.id==driver)
        .ok_or_else(|| HostError::new("DriverRequired", "Unknown driver package"))
}
pub(crate) fn validate_driver(driver: &str) -> Result<(), HostError> { package_spec(driver).map(|_|()) }
pub(crate) fn require_missing(driver: &str, inventory: &Value) -> Result<(), HostError> {
    validate_driver(driver)?;
    let missing=if driver=="newport" {
        inventory["errors"]["newport"].is_null() &&
        (inventory["newport"]["sdk"]["state"]=="missing" || inventory["newport"]["devices"].as_array().is_some_and(|items|items.iter().any(|d|d["driver_state"]=="missing")))
    } else { inventory["errors"]["usb_serial"].is_null() && inventory["usb_serial"][driver]["state"]=="missing" };
    if missing {Ok(())} else {Err(HostError::new("DriverNotMissing","The selected driver is not currently reported missing. Refresh devices before installing."))}
}
pub(crate) struct Installer { state: Mutex<InstallerState> }
struct InstallerState {
    report: Value,
    retained: Option<RetainedInstall>,
}
struct InstallerOwnership {
    files: Vec<std::fs::File>,
    process: Option<Box<dyn InstallProcess>>,
}
struct RetainedInstall(Option<InstallerOwnership>);
impl RetainedInstall {
    fn new(files: Vec<std::fs::File>, process: Option<Box<dyn InstallProcess>>) -> Self {
        Self(Some(InstallerOwnership { files, process }))
    }
    fn file_count(&self) -> usize { self.0.as_ref().unwrap().files.len() }
    fn has_process(&self) -> bool { self.0.as_ref().unwrap().process.is_some() }
}
impl Drop for RetainedInstall {
    fn drop(&mut self) {
        // No discard of a Rust job/outcome may release an unconfirmed installer.
        // The Host stop fence keeps the process alive; Windows recovery is the
        // prescribed lifetime if the original termination cannot be observed.
        static RETAINED: std::sync::OnceLock<Mutex<Vec<InstallerOwnership>>> =
            std::sync::OnceLock::new();
        if let Some(ownership) = self.0.take() {
            RETAINED.get_or_init(|| Mutex::new(Vec::new()))
                .lock().unwrap_or_else(|error| error.into_inner()).push(ownership);
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProcessState { NotStarted, Terminated, Unconfirmed }
pub(crate) struct InstallOutcome {
    result: Result<u32, HostError>,
    process_state: ProcessState,
    retained: Option<RetainedInstall>,
}
impl InstallOutcome {
    fn prelaunch(error: HostError) -> Self {
        Self { result: Err(error), process_state: ProcessState::NotStarted, retained: None }
    }
    fn exited(result: Result<u32, HostError>) -> Self {
        Self { result, process_state: ProcessState::Terminated, retained: None }
    }
    fn unconfirmed(error: HostError, files: Vec<std::fs::File>, process: Option<Box<dyn InstallProcess>>) -> Self {
        Self { result: Err(error), process_state: ProcessState::Unconfirmed,
            retained: Some(RetainedInstall::new(files, process)) }
    }
}
impl Default for Installer {
    fn default() -> Self {
        Self { state: Mutex::new(InstallerState { report: json!({"state":"idle"}), retained: None }) }
    }
}
impl Installer {
    pub fn status(&self) -> Value { self.state.lock().unwrap().report.clone() }
    pub fn admit(&self) -> Result<(), HostError> {
        let status = self.status();
        if matches!(status["state"].as_str(), Some("running" | "unknown")) {
            Err(HostError::new("DriverInstalling", if status["state"] == "unknown" {
                "Driver installer outcome is unknown. Restart Windows before continuing."
            } else { "Driver installation is running. Wait for it to finish." }))
        } else { Ok(()) }
    }
    // The Host holds ordinary admission while checking leases and starting this job.
    pub fn begin(&self, control: &Value, driver: &str) -> Result<(), HostError> {
        validate_driver(driver)?;
        let mut state = self.state.lock().unwrap();
        if matches!(state.report["state"].as_str(), Some("running" | "unknown")) {
            return Err(HostError::new("DriverInstalling", "Driver installation is still running or its outcome is unknown. Check installation status."));
        }
        if !control.as_object().is_some_and(|items| items.values().all(|item| item["state"] == "AVAILABLE")) {
            return Err(HostError::new("DriverInUse", "Disconnect instruments before installing the USB driver."));
        }
        state.report = json!({"state":"running","driver":driver});
        Ok(())
    }
    pub fn finish(&self, result: InstallOutcome) {
        let mut state = self.state.lock().unwrap();
        // A late/duplicate result cannot clear an existing unknown owner/fence.
        if state.report["state"] != "running" { return; }
        let InstallOutcome { result, process_state, retained } = result;
        let driver = state.report["driver"].clone();
        let mut outcome = match result {
            Ok(code @ (0 | 3010)) => json!({"state":"completed","exit_code":code,"restart_required":code==3010}),
            Ok(259) if driver=="ch340" || driver=="cp210x" => json!({"state":"no_change","exit_code":259,"restart_required":false,"message":"Windows did not change a matching device. Refresh devices to check actual driver readiness."}),
            Ok(code) => json!({"state":"failed","exit_code":code,"message":format!("Windows driver installer failed (code {code}).")}),
            Err(error) => json!({"state":if error.code=="DriverInstallUnknown" {"unknown"} else {"failed"},"message":error.message}),
        };
        outcome["driver"] = driver;
        outcome["process_started"] = json!(process_state != ProcessState::NotStarted);
        outcome["process_termination_confirmed"] = json!(process_state == ProcessState::Terminated);
        outcome["resource_pins_retained"] = json!(retained.is_some());
        outcome["protected_file_count"] = json!(retained.as_ref().map_or(0, RetainedInstall::file_count));
        outcome["process_handle_retained"] = json!(retained.as_ref().is_some_and(RetainedInstall::has_process));
        state.retained = retained;
        state.report = outcome;
    }
}
fn verify_file(spec: &PackageFile, bytes: &[u8]) -> Result<(), HostError> {
    if bytes.len() as u64 != spec.size || super::verification::sha256_bytes(bytes)? != spec.sha256 {
        Err(HostError::new("DriverPackage", "A bundled USB driver file is missing or modified. Reinstall the app."))
    } else { Ok(()) }
}
fn package_root() -> Result<PathBuf, HostError> {
    let installed = std::env::current_exe().map_err(|e|HostError::new("DriverPackage",e.to_string()))?
        .parent().ok_or_else(||HostError::new("DriverPackage","Invalid Host location"))?
        .join("drivers");
    if installed.is_dir() { return Ok(installed); }
    #[cfg(debug_assertions)]
    { return Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../drivers")); }
    #[cfg(not(debug_assertions))]
    Err(HostError::new("DriverPackage","Bundled USB drivers are missing."))
}
fn verified_files(root: &Path, spec: &Package) -> Result<Vec<std::fs::File>, HostError> {
    use std::{io::Read, os::windows::fs::OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    let root=root.canonicalize().map_err(|e|HostError::new("DriverPackage",e.to_string()))?;
    let mut files=Vec::new();
    for item in &spec.files {
        let path=root.join(&item.path).canonicalize().map_err(|e|HostError::new("DriverPackage",e.to_string()))?;
        if !path.starts_with(&root) { return Err(HostError::new("DriverPackage","USB driver file escapes package root.")); }
        // Pin every INF/CAT/SYS/DLL dependency against write/delete until the elevated process exits.
        let mut file=std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path)
            .map_err(|e|HostError::new("DriverPackage",e.to_string()))?;
        if file.metadata().map_err(|e|HostError::new("DriverPackage",e.to_string()))?.len()!=item.size {
            return Err(HostError::new("DriverPackage","Invalid USB driver file length."));
        }
        let mut bytes=Vec::new();file.read_to_end(&mut bytes).map_err(|e|HostError::new("DriverPackage",e.to_string()))?;
        verify_file(item,&bytes)?;files.push(file);
    }
    Ok(files)
}
fn installer_arguments(spec: &Package, path: &Path) -> Result<(&'static str, String), HostError> {
    let name=path.to_str().filter(|s|!s.contains(['"','\0'])).ok_or_else(||HostError::new("DriverPackage","Invalid installer path"))?;
    match spec.kind.as_str() {
        "msi" => Ok(("msiexec.exe",format!("/i \"{name}\" /qn /norestart"))),
        "inf" => Ok(("pnputil.exe",format!("/add-driver \"{name}\" /install"))),
        _ => Err(HostError::new("DriverPackage","Unsupported bundled package type")),
    }
}
trait InstallProcess: Send {
    fn wait_for_termination(&mut self) -> bool;
    fn exit_code(&self) -> Result<u32, HostError>;
}
trait InstallLauncher {
    fn launch(
        &self,
        executable: &str,
        parameters: &str,
    ) -> Result<Option<Box<dyn InstallProcess>>, HostError>;
}
struct WindowsLauncher;
struct WindowsProcess(std::os::windows::io::OwnedHandle);
impl InstallProcess for WindowsProcess {
    fn wait_for_termination(&mut self) -> bool {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0,
            System::Threading::{WaitForSingleObject, INFINITE},
        };
        unsafe { WaitForSingleObject(self.0.as_raw_handle(), INFINITE) == WAIT_OBJECT_0 }
    }
    fn exit_code(&self) -> Result<u32, HostError> {
        use std::os::windows::io::AsRawHandle;
        let mut code = 0;
        if unsafe { windows_sys::Win32::System::Threading::GetExitCodeProcess(self.0.as_raw_handle(), &mut code) } == 0 {
            Err(HostError::new("DriverInstallUnknown", "Installer terminated but its exit code is unavailable; restart Windows before continuing."))
        } else {
            Ok(code)
        }
    }
}
impl InstallLauncher for WindowsLauncher {
    fn launch(
        &self,
        executable: &str,
        parameters: &str,
    ) -> Result<Option<Box<dyn InstallProcess>>, HostError> {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::{
            Foundation::GetLastError,
            System::SystemInformation::GetSystemDirectoryW,
            UI::{Shell::{ShellExecuteExW, SHELLEXECUTEINFOW, SEE_MASK_NOCLOSEPROCESS}, WindowsAndMessaging::SW_HIDE},
        };
        fn wide(value: &str) -> Vec<u16> {
            value.encode_utf16().chain(Some(0)).collect()
        }
        let mut system = [0u16; 32768];
        let length = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), system.len() as u32) } as usize;
        if length == 0 || length >= system.len() {
            return Err(HostError::new("DriverInstall", "Windows system directory unavailable."));
        }
        let program = wide(&(String::from_utf16_lossy(&system[..length]) + "\\" + executable));
        let args = wide(parameters);
        let verb = wide("runas");
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = program.as_ptr();
        info.lpParameters = args.as_ptr();
        info.nShow = SW_HIDE;
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let code = unsafe { GetLastError() };
            return Err(HostError::new("DriverInstall", if code == 1223 {
                "Driver installation cancelled in Windows.".into()
            } else {
                format!("Cannot start Windows driver installer (code {code}).")
            }));
        }
        if info.hProcess.is_null() {
            return Ok(None);
        }
        // OwnedHandle closes only when the corresponding process owner is released.
        let handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(info.hProcess) };
        Ok(Some(Box::new(WindowsProcess(handle))))
    }
}
fn install_prepared(
    root: &Path,
    spec: &Package,
    launcher: &dyn InstallLauncher,
) -> InstallOutcome {
    let prepared = (|| {
        let protected = verified_files(root, spec)?;
        let path = root.join(&spec.entry).canonicalize()
            .map_err(|error| HostError::new("DriverPackage", error.to_string()))?;
        let (executable, parameters) = installer_arguments(spec, &path)?;
        Ok::<_, HostError>((protected, executable, parameters))
    })();
    let (protected, executable, parameters) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return InstallOutcome::prelaunch(error),
    };
    let mut process = match launcher.launch(executable, &parameters) {
        Err(error) => return InstallOutcome::prelaunch(error),
        Ok(None) => return InstallOutcome::unconfirmed(
            HostError::new("DriverInstallUnknown", "Installer process handle unavailable; restart Windows before continuing."),
            protected, None,
        ),
        Ok(Some(process)) => process,
    };
    if !process.wait_for_termination() {
        return InstallOutcome::unconfirmed(
            HostError::new("DriverInstallUnknown", "Installer exit unconfirmed; restart Windows before continuing."),
            protected, Some(process),
        );
    }
    // Confirmed termination releases both file pins and process ownership even
    // if the exit code is unreadable. Outcome/readiness still remains unknown.
    InstallOutcome::exited(process.exit_code())
}
pub(crate) fn install(driver: &str) -> InstallOutcome {
    match package_root().and_then(|root| package_spec(driver).map(|spec| (root, spec))) {
        Ok((root, spec)) => install_prepared(&root, &spec, &WindowsLauncher),
        Err(error) => InstallOutcome::prelaunch(error),
    }
}

#[cfg(test)]
pub(crate) fn finite_unconfirmed_install(root: &Path) -> InstallOutcome {
    tests::missing_handle_outcome(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn installation_needs_available_resources_and_cannot_be_started_twice() {
        let job = Installer::default();
        for control in [json!(null),json!({"device:a":{"state":"CONTROLLED"}}),json!({"device:a":{"state":"RETAINED"}})] {
            assert!(job.begin(&control,"newport").is_err());
            assert_eq!(job.status()["state"], "idle");
        }
        job.begin(&json!({"device:a":{"state":"AVAILABLE"}}),"newport").unwrap();
        assert!(job.begin(&json!({}),"cp210x").is_err());
        assert!(job.admit().is_err());
        job.finish(InstallOutcome::exited(Ok(3010)));
        assert_eq!(job.status()["state"],"completed");
        assert_eq!(job.status()["restart_required"],true);
        assert!(job.admit().is_ok());
    }
    #[test]
    fn cancelled_and_failed_installs_are_not_success() {
        let job=Installer::default();
        for code in [1223,1603,1618] {
            job.begin(&json!({}),"newport").unwrap();job.finish(InstallOutcome::exited(Ok(code)));
            assert_eq!(job.status()["state"],"failed");
        }
        job.begin(&json!({}),"newport").unwrap();job.finish(InstallOutcome::exited(Ok(0)));
        assert_eq!(job.status()["state"],"completed");
        assert_eq!(job.status()["restart_required"],false);
    }
    #[test]
    fn pnputil_no_matching_device_is_not_an_installation_success() {
        let job=Installer::default();job.begin(&json!({}),"cp210x").unwrap();job.finish(InstallOutcome::exited(Ok(259)));
        assert_eq!(job.status()["state"],"no_change");
    }
    #[test]
    fn package_changes_are_rejected_before_elevation() {
        for id in ["newport","ch340","cp210x"] {
            for file in package_spec(id).unwrap().files { assert!(verify_file(&file,b"arbitrary installer").is_err()); }
        }
    }
    #[test]
    fn unconfirmed_installer_process_blocks_replay_and_device_admission() {
        let job=Installer::default();job.begin(&json!({}),"ch340").unwrap();
        job.finish(InstallOutcome::unconfirmed(HostError::new("DriverInstallUnknown","Installer exit unconfirmed"), Vec::new(), None));
        assert_eq!(job.status()["state"],"unknown");
        assert_eq!(job.status()["driver"],"ch340");
        assert!(job.admit().is_err());assert!(job.begin(&json!({}),"newport").is_err());
    }
    #[test]
    fn packaged_vendor_bytes_match_the_fixed_digest_without_execution() {
        for id in ["newport","ch340","cp210x"] { verified_files(&package_root().unwrap(),&package_spec(id).unwrap()).unwrap(); }
    }
    #[test]
    fn only_fixed_packages_and_windows_utilities_can_be_selected() {
        let job=Installer::default();
        for id in ["", "../ch340", "C:\\arbitrary.exe", "newport /force"] {
            assert!(job.begin(&json!({}),id).is_err());assert_eq!(job.status()["state"],"idle");
        }
        let (exe,args)=installer_arguments(&package_spec("cp210x").unwrap(),Path::new("C:\\Program Files\\Console\\drivers\\cp210x\\silabser.inf")).unwrap();
        assert_eq!(exe,"pnputil.exe");
        assert_eq!(args,"/add-driver \"C:\\Program Files\\Console\\drivers\\cp210x\\silabser.inf\" /install");
        assert!(installer_arguments(&package_spec("ch340").unwrap(),Path::new("C:\\bad\"file.inf")).is_err());
    }
    #[test]
    fn installer_admission_requires_fresh_positive_missing_driver_metadata() {
        for id in ["ch340","cp210x"] {
            for state in ["ready","not_detected","unavailable","checking"] {
                let mut inventory=json!({"usb_serial":{}});inventory["usb_serial"][id]=json!({"state":state});
                assert!(require_missing(id,&inventory).is_err());
            }
            let mut inventory=json!({"usb_serial":{}});inventory["usb_serial"][id]=json!({"state":"missing"});
            require_missing(id,&inventory).unwrap();
            inventory["errors"]=json!({"usb_serial":"Unconfirmed"});assert!(require_missing(id,&inventory).is_err());
        }
        assert!(require_missing("newport",&json!({"newport":{"sdk":{"state":"ready"},"devices":[]}})).is_err());
        require_missing("newport",&json!({"newport":{"sdk":{"state":"missing"},"devices":[]}})).unwrap();
    }
    #[test]
    fn verified_dependency_is_locked_against_write_for_installer_lifetime() {
        use std::io::Write;
        let root=std::env::temp_dir().join(format!("yang-driver-lock-{}",super::super::registry::new_id().unwrap()));
        std::fs::create_dir_all(root.join("ch340")).unwrap();
        let bytes=b"finite driver file";
        let path=root.join("ch340/file.inf");std::fs::write(&path,bytes).unwrap();
        let spec=Package{id:"ch340".into(),kind:"inf".into(),entry:"ch340/file.inf".into(),files:vec![PackageFile{path:"ch340/file.inf".into(),size:bytes.len() as u64,sha256:super::super::verification::sha256_bytes(bytes).unwrap()}]};
        let protected=verified_files(&root,&spec).unwrap();
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());drop(protected);
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().write_all(b"changed bytes").unwrap();
        assert!(verified_files(&root,&spec).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    enum FiniteLaunch {
        MissingHandle,
        WaitFailure,
        Exited(Option<u32>),
        Rejected,
    }
    struct FiniteLauncher {
        action: FiniteLaunch,
        process_drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    struct FiniteProcess {
        confirmed: bool,
        code: Option<u32>,
        drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl InstallProcess for FiniteProcess {
        fn wait_for_termination(&mut self) -> bool { self.confirmed }
        fn exit_code(&self) -> Result<u32, HostError> {
            assert!(self.confirmed, "Exit code read before confirmed termination");
            self.code.ok_or_else(|| HostError::new("DriverInstallUnknown", "Finite confirmed exit has no code"))
        }
    }
    impl Drop for FiniteProcess {
        fn drop(&mut self) {
            self.drops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    impl FiniteLauncher {
        fn new(action: FiniteLaunch) -> Self {
            Self { action, process_drops: Default::default() }
        }
    }
    impl InstallLauncher for FiniteLauncher {
        fn launch(&self, executable: &str, parameters: &str) -> Result<Option<Box<dyn InstallProcess>>, HostError> {
            assert_eq!(executable, "pnputil.exe");
            assert!(parameters.starts_with("/add-driver \"") && parameters.ends_with("\" /install"));
            let (confirmed, code) = match self.action {
                FiniteLaunch::MissingHandle => return Ok(None),
                FiniteLaunch::Rejected => return Err(HostError::new("DriverInstall", "Finite UAC cancellation")),
                FiniteLaunch::WaitFailure => (false, None),
                FiniteLaunch::Exited(code) => (true, code),
            };
            Ok(Some(Box::new(FiniteProcess { confirmed, code, drops: self.process_drops.clone() })))
        }
    }
    struct LockedFixture { root: PathBuf, path: PathBuf, spec: Package }
    impl LockedFixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("yang-install-retention-{}", super::super::registry::new_id().unwrap()));
            std::fs::create_dir_all(root.join("ch340")).unwrap();
            let bytes = b"finite pinned dependency";
            let path = root.join("ch340/file.inf");
            std::fs::write(&path, bytes).unwrap();
            let spec = Package {
                id: "ch340".into(), kind: "inf".into(), entry: "ch340/file.inf".into(),
                files: vec![PackageFile { path: "ch340/file.inf".into(), size: bytes.len() as u64,
                    sha256: super::super::verification::sha256_bytes(bytes).unwrap() }],
            };
            Self { root, path, spec }
        }
        fn assert_locked(&self) {
            assert!(std::fs::OpenOptions::new().write(true).open(&self.path).is_err(), "Unconfirmed installer lost its write pin");
            assert!(std::fs::remove_file(&self.path).is_err(), "Unconfirmed installer lost its delete pin");
        }
        fn assert_released(&self) {
            drop(std::fs::OpenOptions::new().write(true).open(&self.path).unwrap());
            std::fs::remove_file(&self.path).unwrap();
        }
    }
    impl Drop for LockedFixture {
        fn drop(&mut self) {
            // Unconfirmed pins intentionally outlive the Rust job; test-process exit releases OS handles.
            // Only an ordinary unlocked finite directory can be removed here.
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn assert_unconfirmed_pins(action: FiniteLaunch) {
        let fixture = LockedFixture::new();
        let launcher = FiniteLauncher::new(action);
        let job = Installer::default();
        job.begin(&json!({}), "ch340").unwrap();
        job.finish(install_prepared(&fixture.root, &fixture.spec, &launcher));
        assert_eq!(job.status()["state"], "unknown");
        assert!(job.admit().is_err());
        assert!(job.begin(&json!({}), "newport").is_err());
        assert_eq!(job.status()["process_termination_confirmed"], false);
        assert_eq!(job.status()["resource_pins_retained"], true);
        assert_eq!(job.status()["protected_file_count"], 1);
        assert_eq!(job.status()["process_handle_retained"], matches!(launcher.action, FiniteLaunch::WaitFailure));
        fixture.assert_locked();
        job.finish(InstallOutcome::exited(Ok(0)));
        assert_eq!(job.status()["state"], "unknown", "An unrelated late result cleared retained ownership");
        fixture.assert_locked();
        assert_eq!(launcher.process_drops.load(std::sync::atomic::Ordering::SeqCst), 0);
        drop(job);
        fixture.assert_locked();
        assert_eq!(launcher.process_drops.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    #[test]
    fn unconfirmed_missing_handle_retains_verified_file_pins() {
        assert_unconfirmed_pins(FiniteLaunch::MissingHandle);
    }
    #[test]
    fn unconfirmed_wait_failure_retains_verified_file_and_process_pins() {
        assert_unconfirmed_pins(FiniteLaunch::WaitFailure);
    }
    #[test]
    fn confirmed_termination_without_exit_code_releases_pins_but_keeps_admission_closed() {
        let fixture = LockedFixture::new();
        let launcher = FiniteLauncher::new(FiniteLaunch::Exited(None));
        let job = Installer::default();
        job.begin(&json!({}), "ch340").unwrap();
        job.finish(install_prepared(&fixture.root, &fixture.spec, &launcher));
        assert_eq!(job.status()["state"], "unknown");
        assert!(job.admit().is_err());
        assert_eq!(job.status()["process_termination_confirmed"], true);
        assert_eq!(job.status()["resource_pins_retained"], false);
        assert_eq!(job.status()["process_handle_retained"], false);
        fixture.assert_released();
        assert_eq!(launcher.process_drops.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
    #[test]
    fn confirmed_exit_and_definite_prelaunch_failure_release_verified_pins() {
        for action in [FiniteLaunch::Exited(Some(0)), FiniteLaunch::Rejected] {
            let fixture = LockedFixture::new();
            let launcher = FiniteLauncher::new(action);
            let job = Installer::default();
            job.begin(&json!({}), "ch340").unwrap();
            job.finish(install_prepared(&fixture.root, &fixture.spec, &launcher));
            assert!(job.admit().is_ok());
            fixture.assert_released();
        }
    }

    pub(super) fn missing_handle_outcome(root: &Path) -> InstallOutcome {
        std::fs::create_dir_all(root.join("ch340")).unwrap();
        let bytes = b"finite Host installer dependency";
        std::fs::write(root.join("ch340/file.inf"), bytes).unwrap();
        let spec = Package {
            id: "ch340".into(), kind: "inf".into(), entry: "ch340/file.inf".into(),
            files: vec![PackageFile { path: "ch340/file.inf".into(), size: bytes.len() as u64,
                sha256: super::super::verification::sha256_bytes(bytes).unwrap() }],
        };
        install_prepared(root, &spec, &FiniteLauncher::new(FiniteLaunch::MissingHandle))
    }

}
