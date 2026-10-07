//! Fixed vendor package installation. No caller-supplied paths or commands.
use super::contracts::HostError;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Mutex};

const PACKAGE_HASH: &str = "8f8fafedf3adb94f922e6a1bd7cd14e5f8aee2f3b0f501a8fc2348927adfdb19";
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
    pub fn begin(&self, control: &Value) -> Result<(), HostError> {
        let mut state = self.state.lock().unwrap();
        if matches!(state["state"].as_str(),Some("running"|"unknown")) { return Err(HostError::new("DriverInstalling", "Driver installation is still running or its exit is unconfirmed. Check installation status.")); }
        if !control.as_object().is_some_and(|items| items.values().all(|item| item["state"] == "AVAILABLE")) {
            return Err(HostError::new("DriverInUse", "Disconnect instruments before installing the USB driver."));
        }
        *state = json!({"state":"running"});
        Ok(())
    }
    pub fn finish(&self, result: Result<u32, HostError>) {
        *self.state.lock().unwrap() = match result {
            Ok(code @ (0 | 3010)) => json!({"state":"completed","exit_code":code,"restart_required":code==3010}),
            Ok(code) => json!({"state":"failed","exit_code":code,"message":format!("Windows driver installer failed (code {code}).")}),
            Err(error) => json!({"state":if error.code=="DriverInstallUnknown" {"unknown"} else {"failed"},"message":error.message}),
        };
    }
}
fn verify_package(bytes: &[u8]) -> Result<(), HostError> {
    if bytes.len() != 14_447_104 || super::verification::sha256_bytes(bytes)? != PACKAGE_HASH {
        Err(HostError::new("DriverPackage", "The bundled Newport installer is missing or modified. Reinstall the app."))
    } else { Ok(()) }
}
fn package_path() -> Result<PathBuf, HostError> {
    let installed = std::env::current_exe().map_err(|e|HostError::new("DriverPackage",e.to_string()))?
        .parent().ok_or_else(||HostError::new("DriverPackage","Invalid Host location"))?
        .join("drivers/newport/USBDriverSetup64.msi");
    if installed.is_file() { return Ok(installed); }
    #[cfg(debug_assertions)]
    { return Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../drivers/newport/USBDriverSetup64.msi")); }
    #[cfg(not(debug_assertions))]
    Err(HostError::new("DriverPackage","Bundled Newport installer is missing."))
}
pub(crate) fn install() -> Result<u32, HostError> {
    use std::{io::Read, os::windows::fs::OpenOptionsExt};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, WAIT_OBJECT_0},
        Storage::FileSystem::FILE_SHARE_READ,
        System::{SystemInformation::GetSystemDirectoryW, Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE}},
        UI::{Shell::{ShellExecuteExW, SHELLEXECUTEINFOW, SEE_MASK_NOCLOSEPROCESS}, WindowsAndMessaging::SW_HIDE},
    };
    let path = package_path()?.canonicalize().map_err(|e|HostError::new("DriverPackage",e.to_string()))?;
    // Deny package write/delete for the complete elevation and installation lifetime.
    let mut file = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&path)
        .map_err(|e|HostError::new("DriverPackage",e.to_string()))?;
    if file.metadata().map_err(|e|HostError::new("DriverPackage",e.to_string()))?.len()!=14_447_104 {
        return Err(HostError::new("DriverPackage","Invalid Newport installer length."));
    }
    let mut bytes=Vec::new();file.read_to_end(&mut bytes).map_err(|e|HostError::new("DriverPackage",e.to_string()))?;
    verify_package(&bytes)?;
    fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
    let mut system = [0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), system.len() as u32) } as usize;
    if length==0 || length>=system.len() { return Err(HostError::new("DriverInstall","Windows system directory unavailable.")); }
    let program=wide(&(String::from_utf16_lossy(&system[..length])+"\\msiexec.exe"));
    let name=path.to_str().filter(|s|!s.contains(['"','\0'])).ok_or_else(||HostError::new("DriverPackage","Invalid installer path"))?;
    let args=wide(&format!("/i \"{name}\" /qn /norestart"));let verb=wide("runas");
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
            assert!(job.begin(&control).is_err());
            assert_eq!(job.status()["state"], "idle");
        }
        job.begin(&json!({"device:a":{"state":"AVAILABLE"}})).unwrap();
        assert!(job.begin(&json!({})).is_err());
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
            job.begin(&json!({})).unwrap();job.finish(Ok(code));
            assert_eq!(job.status()["state"],"failed");
        }
        job.begin(&json!({})).unwrap();job.finish(Ok(0));
        assert_eq!(job.status()["state"],"completed");
        assert_eq!(job.status()["restart_required"],false);
    }
    #[test]
    fn package_changes_are_rejected_before_elevation() {
        assert!(verify_package(b"arbitrary installer").is_err());
    }
    #[test]
    fn unconfirmed_installer_process_blocks_replay_and_device_admission() {
        let job=Installer::default();job.begin(&json!({})).unwrap();
        job.finish(Err(HostError::new("DriverInstallUnknown","Installer exit unconfirmed")));
        assert_eq!(job.status()["state"],"unknown");
        assert!(job.admit().is_err());assert!(job.begin(&json!({})).is_err());
    }
    #[test]
    fn packaged_vendor_bytes_match_the_fixed_digest_without_execution() {
        verify_package(&std::fs::read(package_path().unwrap()).unwrap()).unwrap();
    }
}
