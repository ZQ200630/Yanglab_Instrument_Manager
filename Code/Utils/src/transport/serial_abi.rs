use super::{serial::SerialConfig, serial_discovery::DeviceRecord, CloseReport, Deadline};
use crate::{DriverError, DriverResult};
pub trait SerialIo: Send {
    fn configure(&mut self, config: &SerialConfig) -> DriverResult<()>;
    fn write(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<usize>;
    fn read(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>>;
    fn close(&mut self) -> DriverResult<CloseReport>;
    fn has_pending(&self) -> bool;
    /// Inspect already-buffered bytes without a read, purge, or output change.
    fn available(&mut self) -> DriverResult<usize> {
        Err(DriverError::DependencyUnavailable(
            "serial receive-count inspection unavailable".into(),
        ))
    }
}
pub trait SerialBackend: Send + Sync {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>>;
    fn open(&self, port: &str) -> DriverResult<Box<dyn SerialIo>>;
}
pub struct NativeSerial;
impl SerialBackend for NativeSerial {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> {
        #[cfg(windows)]
        {
            native::enumerate()
        }
        #[cfg(not(windows))]
        {
            Err(DriverError::DependencyUnavailable(
                "serial discovery requires Windows".into(),
            ))
        }
    }
    fn open(&self, port: &str) -> DriverResult<Box<dyn SerialIo>> {
        #[cfg(windows)]
        {
            Ok(Box::new(native::NativeIo::open(port)?))
        }
        #[cfg(not(windows))]
        {
            let _ = port;
            Err(DriverError::DependencyUnavailable(
                "serial I/O requires Windows".into(),
            ))
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{mem, ptr};
    use windows_sys::Win32::{
        Devices::{
            Communication::{
                ClearCommError, GetCommState, SetCommState, SetCommTimeouts, COMMTIMEOUTS, COMSTAT,
                DCB,
            },
            DeviceAndDriverInstallation::*,
        },
        Foundation::{
            CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING,
            ERROR_NO_MORE_ITEMS, ERROR_OPERATION_ABORTED, ERROR_SHARING_VIOLATION, GENERIC_READ,
            GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
        },
        Storage::FileSystem::{
            CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
        },
        System::{
            Registry::{RegCloseKey, RegQueryValueExW, KEY_READ, REG_SZ},
            Threading::{CreateEventW, WaitForSingleObject},
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        },
    };
    fn native_error(operation: &str, code: u32, transferred: usize) -> DriverError {
        DriverError::Native {
            operation: operation.into(),
            status: code as i32,
            transferred,
        }
    }
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }
    fn timeout(operation: &str) -> DriverError {
        DriverError::Timeout {
            operation: operation.into(),
            transferred: 0,
        }
    }

    struct Operation {
        bytes: Vec<u8>,
        overlapped: Box<OVERLAPPED>,
    }
    // OVERLAPPED and bytes are heap-stable, never borrowed from a caller. The OS
    // supports completion on another thread. Only NativeIo accesses the operation.
    unsafe impl Send for Operation {}
    impl Operation {
        fn new(bytes: Vec<u8>) -> DriverResult<Self> {
            let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            if event.is_null() {
                return Err(native_error("CreateEventW", unsafe { GetLastError() }, 0));
            }
            let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { mem::zeroed() });
            overlapped.hEvent = event;
            Ok(Self { bytes, overlapped })
        }
    }
    impl Drop for Operation {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.overlapped.hEvent);
            }
        }
    }
    pub(super) struct NativeIo {
        handle: usize,
        pending: Option<Operation>,
    }
    impl NativeIo {
        pub(super) fn open(port: &str) -> DriverResult<Self> {
            let path = wide(port);
            // Zero sharing: never steal an externally owned COM port.
            let handle = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                let code = unsafe { GetLastError() };
                if matches!(code, ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION) {
                    return Err(DriverError::Busy(port.into()));
                }
                return Err(native_error("CreateFileW(COM)", code, 0));
            }
            Ok(Self {
                handle: handle as usize,
                pending: None,
            })
        }
        fn handle(&self) -> HANDLE {
            self.handle as HANDLE
        }
        fn settle(&mut self) -> DriverResult<Option<(Vec<u8>, usize, bool)>> {
            let Some(operation) = self.pending.as_ref() else {
                return Ok(None);
            };
            let mut count = 0;
            let success = unsafe {
                GetOverlappedResult(self.handle(), &*operation.overlapped, &mut count, 0)
            } != 0;
            let code = if success {
                0
            } else {
                unsafe { GetLastError() }
            };
            if !success && code == ERROR_IO_INCOMPLETE {
                return Ok(None);
            }
            if !success && code != ERROR_OPERATION_ABORTED {
                // An unrecognized result does not prove the OS relinquished buffers.
                return Err(native_error("GetOverlappedResult", code, count as usize));
            }
            let operation = self.pending.take().unwrap();
            if count as usize > operation.bytes.len() {
                return Err(DriverError::Protocol(
                    "serial completion count overflow".into(),
                ));
            }
            Ok(Some((operation.bytes.clone(), count as usize, !success)))
        }
        fn transfer(
            &mut self,
            bytes: Vec<u8>,
            read: bool,
            deadline: Deadline,
        ) -> DriverResult<(Vec<u8>, usize)> {
            if self.pending.is_some() {
                return Err(DriverError::Responsibility(
                    "serial operation pending".into(),
                ));
            }
            let timeout_ms = deadline.remaining_millis()?;
            let mut operation = Operation::new(bytes)?;
            let mut count = 0;
            let success = unsafe {
                if read {
                    ReadFile(
                        self.handle(),
                        operation.bytes.as_mut_ptr(),
                        operation.bytes.len() as u32,
                        &mut count,
                        &mut *operation.overlapped,
                    )
                } else {
                    WriteFile(
                        self.handle(),
                        operation.bytes.as_ptr(),
                        operation.bytes.len() as u32,
                        &mut count,
                        &mut *operation.overlapped,
                    )
                }
            } != 0;
            if success {
                if count as usize > operation.bytes.len() {
                    return Err(DriverError::Protocol(
                        "serial immediate count overflow".into(),
                    ));
                }
                return Ok((operation.bytes.clone(), count as usize));
            }
            let code = unsafe { GetLastError() };
            if code != ERROR_IO_PENDING {
                return Err(native_error(
                    if read { "ReadFile" } else { "WriteFile" },
                    code,
                    count as usize,
                ));
            }
            self.pending = Some(operation);
            let event = self.pending.as_ref().unwrap().overlapped.hEvent;
            let wait = unsafe {
                WaitForSingleObject(
                    event,
                    deadline.remaining_millis().unwrap_or(0).min(timeout_ms),
                )
            };
            if wait == WAIT_TIMEOUT || deadline.remaining_millis().is_err() {
                unsafe {
                    CancelIoEx(self.handle(), &*self.pending.as_ref().unwrap().overlapped);
                }
                // Cancellation request is not completion. Keep allocation if still pending.
                let _ = self.settle()?;
                return Err(timeout(if read { "ReadFile" } else { "WriteFile" }));
            }
            match self.settle()? {
                Some((bytes, count, false)) => Ok((bytes, count)),
                Some((_, _, true)) => Err(timeout("serial canceled operation")),
                None => Err(DriverError::Responsibility(
                    "serial completion still pending after wait".into(),
                )),
            }
        }
    }
    impl SerialIo for NativeIo {
        fn available(&mut self) -> DriverResult<usize> {
            if self.handle == 0 {
                return Err(DriverError::Closed);
            }
            if self.pending.is_some() {
                return Err(DriverError::Responsibility(
                    "serial I/O still pending".into(),
                ));
            }
            let mut errors = 0u32;
            let mut state: COMSTAT = unsafe { mem::zeroed() };
            if unsafe { ClearCommError(self.handle(), &mut errors, &mut state) } == 0 {
                return Err(native_error(
                    "ClearCommError inspection",
                    unsafe { GetLastError() },
                    0,
                ));
            }
            if errors != 0 {
                return Err(native_error("serial receive error flags", errors, 0));
            }
            Ok(state.cbInQue as usize)
        }
        fn configure(&mut self, config: &SerialConfig) -> DriverResult<()> {
            let mut dcb: DCB = unsafe { mem::zeroed() };
            dcb.DCBlength = mem::size_of::<DCB>() as u32;
            if unsafe { GetCommState(self.handle(), &mut dcb) } == 0 {
                return Err(native_error("GetCommState", unsafe { GetLastError() }, 0));
            }
            dcb.BaudRate = config.baudrate;
            dcb.ByteSize = 8;
            dcb.Parity = 0;
            dcb.StopBits = 0;
            // Binary, no hardware/software flow control, no abort-on-error;
            // DTR/RTS are explicit per binding, never inherited from another owner.
            dcb._bitfield = 1 | ((config.dtr as u32) << 4) | ((config.rts as u32) << 12);
            if unsafe { SetCommState(self.handle(), &dcb) } == 0 {
                return Err(native_error("SetCommState", unsafe { GetLastError() }, 0));
            }
            let timeouts = COMMTIMEOUTS {
                ReadIntervalTimeout: u32::MAX,
                ReadTotalTimeoutMultiplier: u32::MAX,
                ReadTotalTimeoutConstant: 50,
                WriteTotalTimeoutMultiplier: 0,
                WriteTotalTimeoutConstant: 0,
            };
            if unsafe { SetCommTimeouts(self.handle(), &timeouts) } == 0 {
                return Err(native_error(
                    "SetCommTimeouts",
                    unsafe { GetLastError() },
                    0,
                ));
            }
            Ok(())
        }
        fn write(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<usize> {
            Ok(self.transfer(bytes.to_vec(), false, deadline)?.1)
        }
        fn read(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>> {
            let (mut bytes, count) = self.transfer(vec![0; maximum], true, deadline)?;
            bytes.truncate(count);
            Ok(bytes)
        }
        fn close(&mut self) -> DriverResult<CloseReport> {
            if self.handle == 0 {
                return Ok(CloseReport {
                    released: true,
                    status: None,
                });
            }
            if let Some(operation) = self.pending.as_ref() {
                unsafe {
                    CancelIoEx(self.handle(), &*operation.overlapped);
                }
                if self.settle()?.is_none() {
                    return Ok(CloseReport {
                        released: false,
                        status: Some(ERROR_IO_INCOMPLETE as i32),
                    });
                }
            }
            let success = unsafe { CloseHandle(self.handle()) } != 0;
            let code = if success {
                0
            } else {
                unsafe { GetLastError() }
            };
            if success {
                self.handle = 0;
            }
            Ok(CloseReport {
                released: success,
                status: Some(code as i32),
            })
        }
        fn has_pending(&self) -> bool {
            self.pending.is_some()
        }
    }
    impl Drop for NativeIo {
        fn drop(&mut self) {
            if let Some(operation) = self.pending.take() {
                // Defensive final boundary: never free OS-owned storage if an API
                // caller bypasses SerialSession's retained responsibility registry.
                mem::forget(operation);
            } else if self.handle != 0 {
                unsafe {
                    CloseHandle(self.handle());
                }
            }
        }
    }

    struct Inventory(HDEVINFO);
    impl Drop for Inventory {
        fn drop(&mut self) {
            unsafe {
                SetupDiDestroyDeviceInfoList(self.0);
            }
        }
    }
    fn string(buffer: &[u16]) -> String {
        let length = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..length])
    }
    fn property(set: HDEVINFO, device: &SP_DEVINFO_DATA, property: u32) -> String {
        let mut buffer = vec![0u16; 4096];
        let mut required = 0;
        let success = unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                set,
                device,
                property,
                ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 2) as u32,
                &mut required,
            )
        };
        if success == 0 || required as usize > buffer.len() * 2 {
            String::new()
        } else {
            string(&buffer)
        }
    }
    pub(super) fn enumerate() -> DriverResult<Vec<DeviceRecord>> {
        let set = unsafe {
            SetupDiGetClassDevsW(
                &GUID_DEVCLASS_PORTS,
                ptr::null(),
                ptr::null_mut(),
                DIGCF_PRESENT,
            )
        };
        if set == -1 {
            return Err(native_error(
                "SetupDiGetClassDevsW",
                unsafe { GetLastError() },
                0,
            ));
        }
        let inventory = Inventory(set);
        let mut records = Vec::new();
        for index in 0..4096 {
            let mut device: SP_DEVINFO_DATA = unsafe { mem::zeroed() };
            device.cbSize = mem::size_of::<SP_DEVINFO_DATA>() as u32;
            if unsafe { SetupDiEnumDeviceInfo(inventory.0, index, &mut device) } == 0 {
                let code = unsafe { GetLastError() };
                if code == ERROR_NO_MORE_ITEMS {
                    return Ok(records);
                }
                return Err(native_error("SetupDiEnumDeviceInfo", code, 0));
            }
            // Read the device registry, never CreateFile(COM) during discovery.
            let key = unsafe {
                SetupDiOpenDevRegKey(set, &device, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ)
            };
            if key == INVALID_HANDLE_VALUE {
                continue;
            }
            let mut port = [0u16; 256];
            let mut size = (port.len() * 2) as u32;
            let mut kind = 0;
            let result = unsafe {
                RegQueryValueExW(
                    key,
                    wide("PortName").as_ptr(),
                    ptr::null(),
                    &mut kind,
                    port.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            unsafe {
                RegCloseKey(key);
            }
            if result != 0 || kind != REG_SZ || size as usize > port.len() * 2 {
                continue;
            }
            let port = string(&port);
            if super::super::serial::canonical_com(&port).is_err() {
                continue;
            }
            let mut identity = [0u16; 4096];
            let id_result = unsafe {
                SetupDiGetDeviceInstanceIdW(
                    set,
                    &device,
                    identity.as_mut_ptr(),
                    identity.len() as u32,
                    ptr::null_mut(),
                )
            };
            let instance_id = if id_result != 0 {
                string(&identity)
            } else {
                property(set, &device, SPDRP_HARDWAREID)
            };
            let mut node = device.DevInst;
            let mut parents = Vec::new();
            for _ in 0..8 {
                let mut parent = 0;
                if unsafe { CM_Get_Parent(&mut parent, node, 0) } != CR_SUCCESS {
                    break;
                }
                let mut id = [0u16; 4096];
                if unsafe { CM_Get_Device_IDW(parent, id.as_mut_ptr(), id.len() as u32, 0) }
                    != CR_SUCCESS
                {
                    break;
                }
                parents.push(string(&id));
                node = parent;
            }
            records.push(DeviceRecord {
                port,
                instance_id,
                parents,
                description: property(set, &device, SPDRP_FRIENDLYNAME),
            });
        }
        Err(DriverError::Protocol(
            "serial inventory exceeds 4096 devices".into(),
        ))
    }
}
