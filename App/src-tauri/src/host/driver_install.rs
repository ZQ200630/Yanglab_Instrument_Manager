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
pub(crate) struct Installer { state: Mutex<Value> }
impl Default for Installer {
    fn default() -> Self { Self { state: Mutex::new(json!({"state":"idle"})) } }
}
impl Installer {
    pub fn status(&self) -> Value { self.state.lock().unwrap().clone() }
    pub fn admit(&self) -> Result<(), HostError> {
        if matches!(self.status()["state"].as_str(),Some("running"|"unknown")) {
            Err(HostError::new("DriverInstalling", if self.status()["state"] == "unknown" { "Driver installer exit is unconfirmed. Restart Windows before continuing." } else { "Driver installation is running. Wait for it to finish." }))
        } else { Ok(()) }
    }
    // The Host holds ordinary admission while checking leases and starting this job.
    pub fn begin(&self, control: &Value, driver: &str) -> Result<(), HostError> {
        validate_driver(driver)?;
        let mut state = self.state.lock().unwrap();
        if matches!(state["state"].as_str(),Some("running"|"unknown")) { return Err(HostError::new("DriverInstalling", "Driver installation is still running or its exit is unconfirmed. Check installation status.")); }
        if !control.as_object().is_some_and(|items| items.values().all(|item| item["state"] == "AVAILABLE")) {
            return Err(HostError::new("DriverInUse", "Disconnect instruments before installing the USB driver."));
        }
        *state = json!({"state":"running","driver":driver});
        Ok(())
    }
    pub fn finish(&self, result: Result<u32, HostError>) {
        let mut state=self.state.lock().unwrap();
        let driver=state["driver"].clone();
        let mut outcome = match result {
            Ok(code @ (0 | 3010)) => json!({"state":"completed","exit_code":code,"restart_required":code==3010}),
            Ok(259) if driver=="ch340"||driver=="cp210x" => json!({"state":"no_change","exit_code":259,"restart_required":false,"message":"Windows did not change a matching device. Refresh devices to check actual driver readiness."}),
            Ok(code) => json!({"state":"failed","exit_code":code,"message":format!("Windows driver installer failed (code {code}).")}),
            Err(error) => json!({"state":if error.code=="DriverInstallUnknown" {"unknown"} else {"failed"},"message":error.message}),
        };
        outcome["driver"]=driver;*state=outcome;
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
pub(crate) fn install(driver: &str) -> Result<u32, HostError> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, WAIT_OBJECT_0},
        System::{SystemInformation::GetSystemDirectoryW, Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE}},
        UI::{Shell::{ShellExecuteExW, SHELLEXECUTEINFOW, SEE_MASK_NOCLOSEPROCESS}, WindowsAndMessaging::SW_HIDE},
    };
    let spec=package_spec(driver)?;
    let root=package_root()?;
    let _protected=verified_files(&root,&spec)?;
    let path=root.join(&spec.entry).canonicalize().map_err(|e|HostError::new("DriverPackage",e.to_string()))?;
    let (executable,parameters)=installer_arguments(&spec,&path)?;
    fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
    let mut system = [0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), system.len() as u32) } as usize;
    if length==0 || length>=system.len() { return Err(HostError::new("DriverInstall","Windows system directory unavailable.")); }
    let program=wide(&(String::from_utf16_lossy(&system[..length])+"\\"+executable));
    let args=wide(&parameters);let verb=wide("runas");
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize=std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;info.fMask=SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb=verb.as_ptr();info.lpFile=program.as_ptr();info.lpParameters=args.as_ptr();info.nShow=SW_HIDE;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let code=unsafe { GetLastError() };
        return Err(HostError::new("DriverInstall",if code==1223 {"Driver installation cancelled in Windows.".into()} else {format!("Cannot start Windows driver installer (code {code}).")}));
    }
    if info.hProcess.is_null() { return Err(HostError::new("DriverInstallUnknown","Installer process handle unavailable; restart Windows before continuing.")); }
    let wait=unsafe { WaitForSingleObject(info.hProcess, INFINITE) };let mut code=0;
    let read=unsafe { GetExitCodeProcess(info.hProcess,&mut code) };
    unsafe { CloseHandle(info.hProcess) };
    if wait!=WAIT_OBJECT_0||read==0 { return Err(HostError::new("DriverInstallUnknown","Installer exit unconfirmed; restart Windows before continuing.")); }
    Ok(code)
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
        job.finish(Ok(3010));
        assert_eq!(job.status()["state"],"completed");
        assert_eq!(job.status()["restart_required"],true);
        assert!(job.admit().is_ok());
    }
    #[test]
    fn cancelled_and_failed_installs_are_not_success() {
        let job=Installer::default();
        for code in [1223,1603,1618] {
            job.begin(&json!({}),"newport").unwrap();job.finish(Ok(code));
            assert_eq!(job.status()["state"],"failed");
        }
        job.begin(&json!({}),"newport").unwrap();job.finish(Ok(0));
        assert_eq!(job.status()["state"],"completed");
        assert_eq!(job.status()["restart_required"],false);
    }
    #[test]
    fn pnputil_no_matching_device_is_not_an_installation_success() {
        let job=Installer::default();job.begin(&json!({}),"cp210x").unwrap();job.finish(Ok(259));
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
        job.finish(Err(HostError::new("DriverInstallUnknown","Installer exit unconfirmed")));
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
}
