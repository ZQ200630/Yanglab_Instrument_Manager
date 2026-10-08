//! Read-only prerequisite metadata. No instrument handles or vendor DLL loads.
use crate::{DriverError, DriverResult};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const DOWNLOAD_URL: &str = "https://download.newport.com/#/Software/Newport_USB_Driver/";
pub const INSTALL_NOTE: &str = "Close instrument applications and disconnect/power off Newport USB instruments before vendor installation. Installation requires administrator consent.";
const SDK_HINT: &str = "Install the official Newport USB Driver; NI-VISA is not required.";
const CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverState {
    Ready,
    Missing,
    Unavailable,
    NotDetected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsbDeviceRecord {
    pub instance_id: String,
    pub description: String,
    pub service: String,
    pub problem_code: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriverDevice {
    #[serde(flatten)]
    pub record: UsbDeviceRecord,
    pub driver_state: DriverState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsbFamilyInventory {
    pub state: DriverState,
    pub devices: Vec<DriverDevice>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsbSerialInventory {
    pub ch340: UsbFamilyInventory,
    pub cp210x: UsbFamilyInventory,
}

impl Default for UsbSerialInventory {
    fn default() -> Self {
        Self {
            ch340: UsbFamilyInventory {
                state: DriverState::Unavailable,
                devices: vec![],
            },
            cp210x: UsbFamilyInventory {
                state: DriverState::Unavailable,
                devices: vec![],
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SdkStatus {
    pub state: DriverState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bits: Option<u16>,
    pub message: String,
}

impl SdkStatus {
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            state: DriverState::Unavailable,
            path: None,
            bits: None,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewportInventory {
    pub devices: Vec<DriverDevice>,
    pub sdk: SdkStatus,
    pub download_url: String,
    pub install_note: String,
}

impl Default for NewportInventory {
    fn default() -> Self {
        Self {
            devices: vec![],
            sdk: SdkStatus::unavailable("USB metadata provider unavailable"),
            download_url: DOWNLOAD_URL.into(),
            install_note: INSTALL_NOTE.into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Ch340,
    Cp210x,
    Newport,
}

fn family(instance: &str) -> Option<Family> {
    let upper = instance.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    if bytes.len() < 21 || !upper.starts_with("USB\\VID_") || bytes.get(12..17)? != b"&PID_" {
        return None;
    }
    let vid = u16::from_str_radix(upper.get(8..12)?, 16).ok()?;
    let pid = u16::from_str_radix(upper.get(17..21)?, 16).ok()?;
    if !bytes[8..12].iter().chain(bytes[17..21].iter()).all(u8::is_ascii_hexdigit)
        || !matches!(bytes.get(21), None | Some(b'\\') | Some(b'&'))
    {
        return None;
    }
    match (vid, pid) {
        (0x1a86, 0x7523 | 0x5523) => Some(Family::Ch340),
        (0x10c4, 0xea60 | 0xea63) => Some(Family::Cp210x),
        (0x10c4, 0xea70 | 0xea71 | 0xea7a | 0xea7b)
            if bytes.get(21..25) == Some(b"&MI_")
                && bytes.get(25..27)?.iter().all(u8::is_ascii_hexdigit)
                && bytes.get(27) == Some(&b'\\') =>
        {
            Some(Family::Cp210x)
        }
        (0x104d, 0x100a) => Some(Family::Newport),
        _ => None,
    }
}

fn bounded_records(records: &[UsbDeviceRecord]) -> DriverResult<()> {
    if records.len() > CAPACITY
        || records.iter().any(|r| {
            r.instance_id.encode_utf16().count() >= 512
                || r.description.encode_utf16().count() >= 2048
                || r.service.encode_utf16().count() >= 2048
                || [&r.instance_id, &r.description, &r.service].iter().any(|s| s.contains('\0'))
        })
    {
        return Err(DriverError::Protocol(
            "USB metadata exceeds bounded record/string capacity".into(),
        ));
    }
    Ok(())
}

fn device(record: &UsbDeviceRecord, family: Family) -> DriverDevice {
    let service = record.service.to_ascii_uppercase();
    let supported = match family {
        Family::Ch340 => matches!(
            service.as_str(),
            "CH341SER" | "CH341SER_A" | "CH341SER_A64" | "CH341SER_M64"
        ),
        Family::Cp210x => service == "SILABSER",
        Family::Newport => service == "WINUSB",
    };
    let driver_state = match record.problem_code {
        Some(28) => DriverState::Missing,
        Some(0) if supported => DriverState::Ready,
        _ => DriverState::Unavailable,
    };
    DriverDevice {
        record: record.clone(),
        driver_state,
    }
}

fn family_inventory(records: &[UsbDeviceRecord], wanted: Family) -> UsbFamilyInventory {
    let devices: Vec<_> = records.iter()
        .filter(|r| family(&r.instance_id) == Some(wanted))
        .map(|r| device(r, wanted))
        .collect();
    let state = if devices.is_empty() {
        DriverState::NotDetected
    } else if devices.iter().any(|d| d.driver_state == DriverState::Missing) {
        DriverState::Missing
    } else if devices.iter().any(|d| d.driver_state == DriverState::Unavailable) {
        DriverState::Unavailable
    } else {
        DriverState::Ready
    };
    UsbFamilyInventory { state, devices }
}

pub fn serial_usb_from_records(records: &[UsbDeviceRecord]) -> DriverResult<UsbSerialInventory> {
    bounded_records(records)?;
    Ok(UsbSerialInventory {
        ch340: family_inventory(records, Family::Ch340),
        cp210x: family_inventory(records, Family::Cp210x),
    })
}

pub fn newport_from_records(
    records: &[UsbDeviceRecord],
    sdk: SdkStatus,
) -> DriverResult<NewportInventory> {
    bounded_records(records)?;
    Ok(NewportInventory {
        devices: family_inventory(records, Family::Newport).devices,
        sdk,
        ..NewportInventory::default()
    })
}

// The seams below are private: production always uses Windows metadata and fixed SDK candidates.
fn inspect_pe<R: Read + Seek>(reader: &mut R, length: u64) -> Result<(), String> {
    let mut dos = [0; 64];
    reader.read_exact(&mut dos).map_err(|e| format!("SDK DOS header read: {e}"))?;
    if &dos[..2] != b"MZ" {
        return Err("SDK invalid DOS header".into());
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    if offset > 16 * 1024 * 1024 || offset + 6 > length {
        return Err("SDK invalid PE offset".into());
    }
    reader.seek(SeekFrom::Start(offset)).map_err(|e| format!("SDK PE seek: {e}"))?;
    let mut pe = [0; 6];
    reader.read_exact(&mut pe).map_err(|e| format!("SDK PE header read: {e}"))?;
    if &pe[..4] != b"PE\0\0" {
        return Err("SDK invalid PE signature".into());
    }
    if u16::from_le_bytes(pe[4..].try_into().unwrap()) != 0x8664 {
        return Err("SDK does not match the 64-bit native Worker".into());
    }
    Ok(())
}

fn sdk_from_roots<F>(roots: Result<Vec<PathBuf>, String>, mut probe: F) -> SdkStatus
where
    F: FnMut(&Path) -> Result<Option<PathBuf>, String>,
{
    let roots = match roots {
        Ok(roots) if roots.len() <= 3 => roots,
        Ok(_) => return SdkStatus::unavailable(format!("SDK root capacity exceeded. {SDK_HINT}")),
        Err(error) => {
            return SdkStatus::unavailable(format!("{}. {SDK_HINT}", bounded_message(&error)));
        }
    };
    if roots.iter().any(|root| !root.is_absolute() || root.as_os_str().is_empty()) {
        return SdkStatus::unavailable(format!("SDK installation root is not absolute. {SDK_HINT}"));
    }
    let mut candidates = Vec::new();
    for root in roots {
        let base = root.join("Newport/Newport USB Driver/Bin");
        for candidate in [base.join("usbdll.dll"), base.join("x64/usbdll.dll")] {
            // Windows roots are case-insensitive; preserve the first spelling/order.
            let key = candidate.to_string_lossy().to_ascii_uppercase();
            if !candidates.iter().any(|(old, _)| old == &key) {
                candidates.push((key, candidate));
            }
        }
    }
    let mut failures = Vec::new();
    for (_, candidate) in candidates {
        match probe(&candidate) {
            Ok(Some(path)) if path.is_absolute() => {
                return SdkStatus {
                    state: DriverState::Ready,
                    path: Some(path.to_string_lossy().into_owned()),
                    bits: Some(64),
                    message: "Compatible SDK file found; communication has not been tested.".into(),
                };
            }
            Ok(Some(_)) => failures.push("SDK canonical path is not absolute".into()),
            Ok(None) => (),
            Err(error) => failures.push(bounded_message(&error)),
        }
    }
    if failures.is_empty() {
        SdkStatus {
            state: DriverState::Missing,
            path: None,
            bits: None,
            message: format!("Newport USB SDK is missing. {SDK_HINT}"),
        }
    } else {
        SdkStatus::unavailable(format!("{}. {SDK_HINT}", failures.join("; ")))
    }
}

fn bounded_message(message: &str) -> String {
    message.chars().take(256).collect()
}

fn fixed_sdk_status() -> SdkStatus {
    let roots = ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"].into_iter()
        .filter_map(std::env::var_os)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .collect();
    sdk_from_roots(Ok(roots), |path| {
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("SDK metadata: {error}")),
        };
        if !metadata.is_file() {
            return Err("SDK candidate is not a regular file".into());
        }
        let mut file = std::fs::File::open(path).map_err(|error| format!("SDK open: {error}"))?;
        // Use metadata from the opened file so the offset bound refers to the inspected stream.
        let length = file.metadata().map_err(|error| format!("SDK file metadata: {error}"))?.len();
        inspect_pe(&mut file, length)?;
        let canonical = std::fs::canonicalize(path)
            .map_err(|error| format!("SDK canonicalize: {error}"))?;
        Ok(Some(canonical))
    })
}

trait MetadataApi {
    type Device;
    fn create(&mut self) -> DriverResult<isize>;
    fn next(&mut self, list: isize, index: u32) -> DriverResult<Option<Self::Device>>;
    fn instance(
        &mut self,
        list: isize,
        device: &mut Self::Device,
        buffer: &mut [u16],
    ) -> DriverResult<usize>;
    fn property(
        &mut self,
        list: isize,
        device: &mut Self::Device,
        key: u32,
        buffer: &mut [u16],
    ) -> DriverResult<Option<(u32, usize)>>;
    fn problem(&mut self, device: &mut Self::Device) -> DriverResult<u32>;
    fn destroy(&mut self, list: isize) -> DriverResult<()>;
}

struct MetadataList<'a, A: MetadataApi> {
    api: &'a mut A,
    list: isize,
    close_attempted: bool,
}

impl<A: MetadataApi> MetadataList<'_, A> {
    fn finish(
        &mut self,
        primary: DriverResult<Vec<UsbDeviceRecord>>,
    ) -> DriverResult<Vec<UsbDeviceRecord>> {
        self.close_attempted = true;
        let cleanup = self.api.destroy(self.list);
        match (primary, cleanup) {
            (Ok(records), Ok(())) => Ok(records),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(primary), Err(cleanup)) => Err(DriverError::Responsibility(
                format!("{primary}; metadata cleanup: {cleanup}"),
            )),
        }
    }
}

impl<A: MetadataApi> Drop for MetadataList<'_, A> {
    fn drop(&mut self) {
        if !self.close_attempted {
            self.close_attempted = true;
            let _ = self.api.destroy(self.list);
        }
    }
}

fn terminated_string(buffer: &[u16], units: usize) -> DriverResult<String> {
    if units == 0 || units > buffer.len() || buffer[units - 1] != 0
        || buffer[..units - 1].contains(&0)
    {
        return Err(DriverError::Protocol("USB metadata invalid string length/terminator".into()));
    }
    String::from_utf16(&buffer[..units - 1])
        .map_err(|_| DriverError::Protocol("USB metadata invalid UTF-16".into()))
}

fn property<A: MetadataApi>(
    list: &mut MetadataList<'_, A>,
    device: &mut A::Device,
    key: u32,
) -> DriverResult<String> {
    let mut buffer = [0u16; 2048];
    match list.api.property(list.list, device, key, &mut buffer)? {
        None => Ok(String::new()),
        Some((kind, bytes)) => {
            if kind != 1 || bytes % 2 != 0 {
                return Err(DriverError::Protocol("USB metadata invalid REG_SZ property".into()));
            }
            terminated_string(&buffer, bytes / 2)
        }
    }
}

fn metadata_with<A: MetadataApi>(
    api: &mut A,
    wanted: &[Family],
) -> DriverResult<Vec<UsbDeviceRecord>> {
    let handle = api.create()?;
    let mut list = MetadataList { api, list: handle, close_attempted: false };
    let primary = (|| {
        let mut records = Vec::new();
        for index in 0..=CAPACITY {
            let Some(mut device) = list.api.next(list.list, index as u32)? else {
                return Ok(records);
            };
            if index == CAPACITY {
                return Err(DriverError::Protocol("USB metadata enumeration capacity exceeded".into()));
            }
            let mut buffer = [0u16; 512];
            let units = list.api.instance(list.list, &mut device, &mut buffer)?;
            let instance_id = terminated_string(&buffer, units)?;
            if !family(&instance_id).is_some_and(|family| wanted.contains(&family)) {
                continue;
            }
            let mut description = property(&mut list, &mut device, 12)?;
            if description.is_empty() {
                description = property(&mut list, &mut device, 0)?;
            }
            let service = property(&mut list, &mut device, 4)?;
            let problem_code = Some(list.api.problem(&mut device)?);
            records.push(UsbDeviceRecord { instance_id, description, service, problem_code });
        }
        unreachable!()
    })();
    list.finish(primary)
}

#[cfg(windows)]
mod windows_metadata {
    use super::*;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_Status, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo,
        SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW,
        CR_SUCCESS, DIGCF_ALLCLASSES, DIGCF_PRESENT, SP_DEVINFO_DATA,
    };
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_INVALID_DATA, ERROR_NO_MORE_ITEMS};

    pub(super) struct NativeMetadata;

    fn native_error(operation: &str, status: u32) -> DriverError {
        DriverError::Native { operation: operation.into(), status: status as i32, transferred: 0 }
    }

    impl MetadataApi for NativeMetadata {
        type Device = SP_DEVINFO_DATA;

        fn create(&mut self) -> DriverResult<isize> {
            let usb = [b'U' as u16, b'S' as u16, b'B' as u16, 0];
            // Present devices of every class include adapters with no installed driver.
            let list = unsafe {
                SetupDiGetClassDevsW(
                    std::ptr::null(), usb.as_ptr(), std::ptr::null_mut(),
                    DIGCF_PRESENT | DIGCF_ALLCLASSES,
                )
            };
            if list == -1 {
                Err(native_error("SetupDiGetClassDevsW", unsafe { GetLastError() }))
            } else {
                Ok(list)
            }
        }

        fn next(&mut self, list: isize, index: u32) -> DriverResult<Option<Self::Device>> {
            // The bindings supply the correct target-specific ABI/layout.
            let mut device: SP_DEVINFO_DATA = unsafe { std::mem::zeroed() };
            device.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
            if unsafe { SetupDiEnumDeviceInfo(list, index, &mut device) } != 0 {
                return Ok(Some(device));
            }
            let error = unsafe { GetLastError() };
            if error == ERROR_NO_MORE_ITEMS {
                Ok(None)
            } else {
                Err(native_error("SetupDiEnumDeviceInfo", error))
            }
        }

        fn instance(
            &mut self,
            list: isize,
            device: &mut Self::Device,
            buffer: &mut [u16],
        ) -> DriverResult<usize> {
            let mut needed = 0;
            if unsafe {
                SetupDiGetDeviceInstanceIdW(
                    list, device, buffer.as_mut_ptr(), buffer.len() as u32, &mut needed,
                )
            } == 0 {
                return Err(native_error("SetupDiGetDeviceInstanceIdW", unsafe { GetLastError() }));
            }
            Ok(needed as usize)
        }

        fn property(
            &mut self,
            list: isize,
            device: &mut Self::Device,
            key: u32,
            buffer: &mut [u16],
        ) -> DriverResult<Option<(u32, usize)>> {
            let mut kind = 0;
            let mut needed = 0;
            if unsafe {
                SetupDiGetDeviceRegistryPropertyW(
                    list, device, key, &mut kind, buffer.as_mut_ptr().cast(),
                    (buffer.len() * 2) as u32, &mut needed,
                )
            } == 0 {
                let error = unsafe { GetLastError() };
                if error == ERROR_INVALID_DATA {
                    return Ok(None);
                }
                return Err(native_error("SetupDiGetDeviceRegistryPropertyW", error));
            }
            Ok(Some((kind, needed as usize)))
        }

        fn problem(&mut self, device: &mut Self::Device) -> DriverResult<u32> {
            let mut status = 0;
            let mut problem = 0;
            let result = unsafe {
                CM_Get_DevNode_Status(&mut status, &mut problem, device.DevInst, 0)
            };
            if result == CR_SUCCESS {
                Ok(problem)
            } else {
                Err(native_error("CM_Get_DevNode_Status(configret)", result))
            }
        }

        fn destroy(&mut self, list: isize) -> DriverResult<()> {
            if unsafe { SetupDiDestroyDeviceInfoList(list) } == 0 {
                Err(native_error("SetupDiDestroyDeviceInfoList", unsafe { GetLastError() }))
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(windows)]
fn system_records(wanted: &[Family]) -> DriverResult<Vec<UsbDeviceRecord>> {
    metadata_with(&mut windows_metadata::NativeMetadata, wanted)
}

#[cfg(not(windows))]
fn system_records(_wanted: &[Family]) -> DriverResult<Vec<UsbDeviceRecord>> {
    Err(DriverError::DependencyUnavailable("USB prerequisite metadata requires Windows".into()))
}

/// Explicit metadata request; never loads the SDK or opens an instrument.
pub fn system_usb_serial() -> DriverResult<UsbSerialInventory> {
    serial_usb_from_records(&system_records(&[Family::Ch340, Family::Cp210x])?)
}

/// Explicit metadata request; SDK inspection reads only fixed approved candidate files.
pub fn system_newport() -> DriverResult<NewportInventory> {
    newport_from_records(&system_records(&[Family::Newport])?, fixed_sdk_status())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};
    fn pe(machine: u16) -> Vec<u8> {
        let mut bytes = vec![0; 134];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&128u32.to_le_bytes());
        bytes[128..132].copy_from_slice(b"PE\0\0");
        bytes[132..].copy_from_slice(&machine.to_le_bytes());
        bytes
    }
    #[test]
    fn usb_drivers_pe_is_bounded_and_rejects_incompatible_or_malformed_headers() {
        let valid = pe(0x8664);
        assert!(inspect_pe(&mut Cursor::new(valid.clone()), valid.len() as u64).is_ok());
        for machine in [0x14c, 0xaa64, 0] {
            assert!(inspect_pe(&mut Cursor::new(pe(machine)), 134).is_err());
        }
        for bytes in [vec![], vec![0; 64], pe(0x8664)[..133].to_vec()] {
            assert!(inspect_pe(&mut Cursor::new(&bytes), bytes.len() as u64).is_err());
        }
        for offset in [u32::MAX, 16 * 1024 * 1024 + 1, 133] {
            let mut bytes = valid.clone();
            bytes[60..64].copy_from_slice(&offset.to_le_bytes());
            assert!(inspect_pe(&mut Cursor::new(bytes), 134).is_err());
        }
        let mut bytes = valid;
        bytes[128] = b'X';
        assert!(inspect_pe(&mut Cursor::new(bytes), 134).is_err());
    }
    struct BrokenIo(bool);
    impl Read for BrokenIo {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0 { return Err(io::Error::other("read denied")); }
            buf.copy_from_slice(&pe(0x8664)[..buf.len()]);
            Ok(buf.len())
        }
    }
    impl Seek for BrokenIo {
        fn seek(&mut self, _: SeekFrom) -> io::Result<u64> { Err(io::Error::other("seek denied")) }
    }
    #[test]
    fn usb_drivers_pe_read_and_seek_failures_are_not_missing() {
        for failure in [true, false] { assert!(inspect_pe(&mut BrokenIo(failure), 134).is_err()); }
    }
    #[test]
    fn usb_drivers_fixed_sdk_candidates_distinguish_absence_errors_and_later_compatible_file() {
        let root = PathBuf::from(if cfg!(windows) { "C:\\VendorRoot" } else { "/VendorRoot" });
        let roots = || Ok(vec![root.clone(), root.clone()]);
        let mut seen = vec![];
        let absent = sdk_from_roots(roots(), |p| { seen.push(p.to_path_buf()); Ok(None) });
        assert_eq!(absent.state, DriverState::Missing);
        assert_eq!(seen, vec![root.join("Newport/Newport USB Driver/Bin/usbdll.dll"), root.join("Newport/Newport USB Driver/Bin/x64/usbdll.dll")]);
        let bad = sdk_from_roots(roots(), |_| Err("access denied".into()));
        assert_eq!(bad.state, DriverState::Unavailable);
        let mut calls = 0;
        let ready = sdk_from_roots(roots(), |p| {
            calls += 1;
            if calls == 1 { Err("incompatible".into()) } else { Ok(Some(p.to_path_buf())) }
        });
        assert_eq!(ready.state, DriverState::Ready);
        assert_eq!(ready.bits, Some(64));
        assert_eq!(ready.message, "Compatible SDK file found; communication has not been tested.");
        assert_eq!(calls, 2);
        let relative = sdk_from_roots(Ok(vec![PathBuf::from("relative")]), |_| panic!("must not probe arbitrary relative root"));
        assert_eq!(relative.state, DriverState::Unavailable);
        assert_eq!(sdk_from_roots(Err("invalid environment".into()), |_| panic!()).state, DriverState::Unavailable);
        assert_eq!(sdk_from_roots(Ok(vec![]), |_| panic!()).state, DriverState::Missing);
        assert_eq!(sdk_from_roots(roots(), |_| Ok(Some(PathBuf::from("relative")))).state, DriverState::Unavailable);
    }
    struct FakeApi {
        count: usize,
        id: &'static str,
        destroy_fail: bool,
        panic_next: bool,
        close: usize,
        fail: Option<&'static str>,
        absent: bool,
        malformed: Option<&'static str>,
        calls: Vec<String>,
    }
    impl FakeApi {
        fn new(count: usize) -> Self { Self {
                count,
                id: "USB\\VID_1A86&PID_7523\\finite",
                destroy_fail: false,
                panic_next: false,
                close: 0,
                fail: None,
                absent: false,
                malformed: None,
                calls: vec![],
            } }
        fn error(&self, operation: &str) -> DriverResult<()> {
            if self.fail == Some(operation) {
                Err(DriverError::Native { operation: operation.into(), status: 5, transferred: 0 })
            } else {
                Ok(())
            }
        }
        fn string(&self, value: &str, buffer: &mut [u16], name: &str) -> usize {
            let words: Vec<_> = value.encode_utf16().chain([0]).collect();
            buffer[..words.len()].copy_from_slice(&words);
            match self.malformed {
                Some("overflow") => buffer.len() + 1,
                Some("unterminated") => { buffer[words.len()-1] = b'x' as u16; words.len() },
                Some("utf16") => { buffer[0] = 0xd800; words.len() },
                                Some("service_overflow") if name == "property" => buffer.len() + 1,
                Some("property_unterminated") if name == "property" => { buffer[words.len()-1] = 1; words.len() },
                Some("property_utf16") if name == "property" => { buffer[0] = 0xd800; words.len() },
                Some("interior_null") if name == "property" => { buffer[1] = 0; words.len() },
                _ => words.len(),
            }
        }
    }
    impl MetadataApi for FakeApi {
        type Device = u32;
        fn create(&mut self) -> DriverResult<isize> { self.calls.push("create".into()); self.error("create")?; Ok(17) }
        fn next(&mut self, list: isize, index: u32) -> DriverResult<Option<u32>> {
            assert_eq!(list, 17);
            if self.panic_next { panic!("finite API panic"); }
            self.calls.push(format!("next:{index}"));
            self.error("next")?;
            Ok(((index as usize) < self.count).then_some(index))
        }
        fn instance(&mut self, _: isize, _: &mut u32, buffer: &mut [u16]) -> DriverResult<usize> {
            assert_eq!(buffer.len(), 512); self.error("instance")?;
            Ok(self.string(self.id, buffer, "instance"))
        }
        fn property(&mut self, _: isize, _: &mut u32, key: u32, buffer: &mut [u16]) -> DriverResult<Option<(u32, usize)>> {
            assert_eq!(buffer.len(), 2048);
            self.calls.push(format!("property:{key}"));
            self.error("property")?;
            if self.absent && key != 0 { return Ok(None); }
            let value = if key == 4 { "CH341SER" } else { "description" };
            let len = self.string(value, buffer, "property") * 2;
            Ok(Some((if self.malformed == Some("type") { 3 } else { 1 }, if self.malformed == Some("odd") { len-1 } else { len })))
        }
        fn problem(&mut self, _: &mut u32) -> DriverResult<u32> { self.error("problem")?; Ok(28) }
        fn destroy(&mut self, _: isize) -> DriverResult<()> {
            self.close += 1;
            self.calls.push("destroy".into());
            if self.destroy_fail {
                return Err(DriverError::Native { operation: "destroy".into(), status: 5, transferred: 0 });
            }
            self.error("destroy")
        }
    }
    #[test]
    fn usb_drivers_metadata_success_absence_and_capacity_always_release_the_list() {
        for count in [0, 1, 4096] {
            let mut api = FakeApi::new(count);
            let records = metadata_with(&mut api, &[Family::Ch340]).unwrap();
            assert_eq!(records.len(), count);
            assert_eq!(api.close, 1);
            assert_eq!(api.calls.last().unwrap(), "destroy");
        }
        let mut api = FakeApi::new(4097);
        assert!(metadata_with(&mut api, &[Family::Ch340]).is_err());
        assert_eq!(api.close, 1);
        assert!(api.calls.iter().any(|s| s == "next:4096"));
        let mut api = FakeApi::new(1);
        api.absent = true;
        let records = metadata_with(&mut api, &[Family::Ch340]).unwrap();
        assert_eq!(records[0].description, "description");
        assert_eq!(records[0].service, "");
        assert!(api.calls.iter().any(|s| s == "property:0"));
    }
    #[test]
    fn usb_drivers_metadata_failures_and_malformed_properties_never_become_missing() {
        for operation in ["create", "next", "instance", "property", "problem", "destroy"] {
            let mut api = FakeApi::new(1);
            api.fail = Some(operation);
            let error = metadata_with(&mut api, &[Family::Ch340]).unwrap_err();
            assert!(error.to_string().contains(operation));
            assert_eq!(api.close, if operation == "create" { 0 } else { 1 });
        }
        for malformed in ["overflow", "unterminated", "utf16", "service_overflow", "type", "odd", "property_unterminated", "property_utf16", "interior_null"] {
            let mut api = FakeApi::new(1);
            api.malformed = Some(malformed);
            assert!(metadata_with(&mut api, &[Family::Ch340]).is_err(), "{malformed}");
            assert_eq!(api.close, 1);
        }
    }
    #[test]
    fn usb_drivers_metadata_combines_cleanup_failure_and_drop_covers_unwind() {
        let mut api = FakeApi::new(1);
        api.fail = Some("problem");
        api.destroy_fail = true;
        let error = metadata_with(&mut api, &[Family::Ch340]).unwrap_err().to_string();
        assert!(error.contains("problem"));
        assert!(error.contains("destroy"));
        assert_eq!(api.close, 1);
        let mut api = FakeApi::new(1);
        api.panic_next = true;
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            metadata_with(&mut api, &[Family::Ch340])
        })).is_err());
        assert_eq!(api.close, 1);
    }
    #[test]
    fn usb_drivers_ignored_ids_and_composite_parents_do_not_fetch_properties() {
        for id in ["USB\\VID_9999&PID_0000\\a", "USB\\VID_10C4&PID_EA70\\parent"] {
            let mut api = FakeApi::new(1);
            api.id = id;
            api.fail = Some("property");
            let records = metadata_with(&mut api, &[Family::Ch340, Family::Cp210x]).unwrap();
            assert!(records.is_empty());
            assert!(!api.calls.iter().any(|s| s.starts_with("property:")));
            assert_eq!(api.close, 1);
        }
    }
    struct ExactPeReader { bytes: Cursor<Vec<u8>>, reads: Vec<usize>, seeks: Vec<u64> }
    impl Read for ExactPeReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.reads.push(buffer.len());
            self.bytes.read(buffer)
        }
    }
    impl Seek for ExactPeReader {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            if let SeekFrom::Start(offset) = position { self.seeks.push(offset); }
            self.bytes.seek(position)
        }
    }
    #[test]
    fn usb_drivers_pe_reads_only_dos_and_machine_headers() {
        let mut reader = ExactPeReader { bytes: Cursor::new(pe(0x8664)), reads: vec![], seeks: vec![] };
        inspect_pe(&mut reader, 134).unwrap();
        assert_eq!(reader.reads, vec![64, 6]);
        assert_eq!(reader.seeks, vec![128]);
    }
}
