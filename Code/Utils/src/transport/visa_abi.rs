//! Reviewed against IVI Foundation VISA Win64 headers (2026-10-06).
//! `ViSession/ViObject/ViFindList/ViAttr/ViUInt32` are u32; `ViStatus` i32;
//! `ViUInt16` u16; x64 `ViAttrState` u64. `_VI_FUNC` uses the Windows x64 ABI.
//! Signatures: OpenDefaultRM(*u32); FindRsrc(u32,*c_char,*u32,*u32,*c_char);
//! FindNext(u32,*c_char); ParseRsrcEx(u32,*c_char,*u16,*u16,*c_char,*c_char,*c_char);
//! Open(u32,*c_char,u32,u32,*u32); Close(u32); SetAttribute(u32,u32,u64);
//! Read(u32,*u8,u32,*u32); Write(u32,*const u8,u32,*u32). All return i32.
//! No reset, clear, trigger, lock stealing or output-changing export is bound.
use crate::{DriverError, DriverResult};
use std::sync::Arc;
pub const VI_ERROR_RSRC_BUSY: i32 = 0xBFFF0072u32 as i32;
pub const VI_ERROR_TMO: i32 = 0xBFFF0015u32 as i32;
pub const VI_ERROR_RSRC_NFOUND: i32 = 0xBFFF0011u32 as i32;
pub const VI_EXCLUSIVE_LOCK: u32 = 1;
pub const VI_ATTR_TMO_VALUE: u32 = 0x3FFF001A;
pub const VI_ATTR_TERMCHAR_EN: u32 = 0x3FFF0038;
pub const VI_SUCCESS_TERM_CHAR: i32 = 0x3FFF0005;
pub const VI_SUCCESS_MAX_CNT: i32 = 0x3FFF0006;
pub trait VisaApi: Send + Sync {
    fn open_manager(&self) -> (i32, u32);
    fn parse(&self, manager: u32, name: &str) -> DriverResult<String>;
    fn find(&self, manager: u32) -> (i32, u32, u32, DriverResult<String>);
    fn find_next(&self, list: u32) -> DriverResult<String>;
    fn open(&self, manager: u32, name: &str, mode: u32, timeout_ms: u32) -> (i32, u32);
    fn close(&self, handle: u32) -> i32;
    fn set_timeout(&self, handle: u32, milliseconds: u32) -> i32;
    fn set_termination_enabled(&self, handle: u32, enabled: bool) -> i32;
    fn write(&self, handle: u32, bytes: &[u8]) -> (i32, u32);
    fn read(&self, handle: u32, bytes: &mut [u8]) -> (i32, u32);
}
pub fn load_system() -> DriverResult<Arc<dyn VisaApi>> {
    #[cfg(all(windows, target_pointer_width = "64"))]
    {
        Ok(Arc::new(native::NativeVisa::load()?))
    }
    #[cfg(not(all(windows, target_pointer_width = "64")))]
    {
        Err(DriverError::DependencyUnavailable(
            "VISA backend requires 64-bit Windows".into(),
        ))
    }
}

#[cfg(all(windows, target_pointer_width = "64"))]
mod native {
    use super::*;
    use std::{
        ffi::{c_char, CString},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{FreeLibrary, HMODULE},
        System::{
            LibraryLoader::{
                GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            },
            SystemInformation::GetSystemDirectoryW,
        },
    };

    type OpenManager = unsafe extern "system" fn(*mut u32) -> i32;
    type Find =
        unsafe extern "system" fn(u32, *const c_char, *mut u32, *mut u32, *mut c_char) -> i32;
    type Next = unsafe extern "system" fn(u32, *mut c_char) -> i32;
    type Parse = unsafe extern "system" fn(
        u32,
        *const c_char,
        *mut u16,
        *mut u16,
        *mut c_char,
        *mut c_char,
        *mut c_char,
    ) -> i32;
    type Open = unsafe extern "system" fn(u32, *const c_char, u32, u32, *mut u32) -> i32;
    type Close = unsafe extern "system" fn(u32) -> i32;
    type SetAttribute = unsafe extern "system" fn(u32, u32, u64) -> i32;
    type Read = unsafe extern "system" fn(u32, *mut u8, u32, *mut u32) -> i32;
    type Write = unsafe extern "system" fn(u32, *const u8, u32, *mut u32) -> i32;

    // An integer is the opaque loader handle, not a pointer dereferenced by Rust.
    // Arc<NativeVisa> is held through every call and retained for every live handle.
    struct Module(usize);
    impl Drop for Module {
        fn drop(&mut self) {
            unsafe {
                FreeLibrary(self.0 as HMODULE);
            }
        }
    }
    pub(super) struct NativeVisa {
        _module: Module,
        open_manager: OpenManager,
        find: Find,
        next: Next,
        parse: Parse,
        open: Open,
        close: Close,
        set_attribute: SetAttribute,
        read: Read,
        write: Write,
    }
    fn unavailable(message: impl Into<String>) -> DriverError {
        DriverError::DependencyUnavailable(message.into())
    }
    fn text(buffer: &[c_char]) -> DriverResult<String> {
        let end = buffer
            .iter()
            .position(|c| *c == 0)
            .ok_or_else(|| DriverError::Protocol("unterminated VISA string".into()))?;
        let bytes: Vec<u8> = buffer[..end].iter().map(|c| *c as u8).collect();
        String::from_utf8(bytes)
            .map_err(|_| DriverError::Protocol("VISA string is not UTF-8".into()))
    }
    fn string(value: &str) -> DriverResult<CString> {
        CString::new(value).map_err(|_| DriverError::Invalid("NUL in VISA resource".into()))
    }
    fn check(operation: &str, code: i32) -> DriverResult<()> {
        if code >= 0 {
            Ok(())
        } else {
            Err(DriverError::Native {
                operation: operation.into(),
                status: code,
                transferred: 0,
            })
        }
    }
    impl NativeVisa {
        pub(super) fn load() -> DriverResult<Self> {
            let mut path = vec![0u16; 32768];
            // Buffer and length agree; returned length excludes NUL.
            let length =
                unsafe { GetSystemDirectoryW(path.as_mut_ptr(), path.len() as u32) } as usize;
            if length == 0 || length >= path.len() {
                return Err(unavailable("Cannot resolve Windows System32"));
            }
            path.truncate(length);
            path.extend("\\visa64.dll".encode_utf16());
            path.push(0);
            // Absolute system path, and only its directory/System32 may resolve dependencies.
            let handle = unsafe {
                LoadLibraryExW(
                    path.as_ptr(),
                    ptr::null_mut(),
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
                )
            };
            if handle.is_null() {
                return Err(unavailable(
                    "VISA x64 is unavailable; install a supported vendor VISA separately",
                ));
            }
            let module = Module(handle as usize);
            macro_rules! bind {
                ($name:literal, $kind:ty) => {{
                    // Each named export's exact declaration is reviewed above; no generic cast.
                    let pointer = unsafe { GetProcAddress(handle, concat!($name, "\0").as_ptr()) }
                        .ok_or_else(|| unavailable(concat!("VISA missing export ", $name)))?;
                    unsafe {
                        std::mem::transmute::<unsafe extern "system" fn() -> isize, $kind>(pointer)
                    }
                }};
            }
            Ok(Self {
                open_manager: bind!("viOpenDefaultRM", OpenManager),
                find: bind!("viFindRsrc", Find),
                next: bind!("viFindNext", Next),
                parse: bind!("viParseRsrcEx", Parse),
                open: bind!("viOpen", Open),
                close: bind!("viClose", Close),
                set_attribute: bind!("viSetAttribute", SetAttribute),
                read: bind!("viRead", Read),
                write: bind!("viWrite", Write),
                _module: module,
            })
        }
    }
    impl VisaApi for NativeVisa {
        fn open_manager(&self) -> (i32, u32) {
            let mut handle = 0;
            let code = unsafe { (self.open_manager)(&mut handle) };
            (code, handle)
        }
        fn parse(&self, manager: u32, name: &str) -> DriverResult<String> {
            let name = string(name)?;
            let (mut kind, mut number) = (0u16, 0u16);
            let (mut class, mut expanded, mut alias) = ([0; 256], [0; 256], [0; 256]);
            let code = unsafe {
                (self.parse)(
                    manager,
                    name.as_ptr(),
                    &mut kind,
                    &mut number,
                    class.as_mut_ptr(),
                    expanded.as_mut_ptr(),
                    alias.as_mut_ptr(),
                )
            };
            check("viParseRsrcEx", code)?;
            text(&expanded)
        }
        fn find(&self, manager: u32) -> (i32, u32, u32, DriverResult<String>) {
            let (mut list, mut count) = (0, 0);
            let mut buffer = [0; 256];
            let code = unsafe {
                (self.find)(
                    manager,
                    c"?*".as_ptr(),
                    &mut list,
                    &mut count,
                    buffer.as_mut_ptr(),
                )
            };
            (code, list, count, text(&buffer))
        }
        fn find_next(&self, list: u32) -> DriverResult<String> {
            let mut buffer = [0; 256];
            let code = unsafe { (self.next)(list, buffer.as_mut_ptr()) };
            check("viFindNext", code)?;
            text(&buffer)
        }
        fn open(&self, manager: u32, name: &str, mode: u32, timeout_ms: u32) -> (i32, u32) {
            let Ok(name) = string(name) else {
                return (0xBFFF0012u32 as i32, 0);
            };
            let mut handle = 0;
            let code =
                unsafe { (self.open)(manager, name.as_ptr(), mode, timeout_ms, &mut handle) };
            (code, handle)
        }
        fn close(&self, handle: u32) -> i32 {
            unsafe { (self.close)(handle) }
        }
        fn set_timeout(&self, handle: u32, milliseconds: u32) -> i32 {
            unsafe { (self.set_attribute)(handle, VI_ATTR_TMO_VALUE, milliseconds as u64) }
        }
        fn set_termination_enabled(&self, handle: u32, enabled: bool) -> i32 {
            unsafe { (self.set_attribute)(handle, VI_ATTR_TERMCHAR_EN, u64::from(enabled)) }
        }
        fn write(&self, handle: u32, bytes: &[u8]) -> (i32, u32) {
            let Ok(length) = u32::try_from(bytes.len()) else {
                return (0xBFFF0012u32 as i32, 0);
            };
            let mut count = 0;
            let code = unsafe { (self.write)(handle, bytes.as_ptr(), length, &mut count) };
            (code, count)
        }
        fn read(&self, handle: u32, bytes: &mut [u8]) -> (i32, u32) {
            let Ok(length) = u32::try_from(bytes.len()) else {
                return (0xBFFF0012u32 as i32, 0);
            };
            let mut count = 0;
            let code = unsafe { (self.read)(handle, bytes.as_mut_ptr(), length, &mut count) };
            (code, count)
        }
    }
}
