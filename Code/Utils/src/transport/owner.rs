use crate::{DriverError, DriverResult};
pub struct OwnerGuard {
    handle: usize,
}
impl OwnerGuard {
    pub fn acquire() -> DriverResult<Self> {
        Self::acquire_named("Global\\YangLabInstrumentHost")
    }
    pub fn acquire_named(name: &str) -> DriverResult<Self> {
        if name != "Global\\YangLabInstrumentHost" && !name.starts_with("Global\\YangLab-test-") {
            return Err(DriverError::Invalid("unsupported owner namespace".into()));
        }
        if name.len() > 256 || name.contains('\0') {
            return Err(DriverError::Invalid("invalid owner name".into()));
        }
        #[cfg(windows)]
        {
            Ok(Self {
                handle: native::acquire(name)?,
            })
        }
        #[cfg(not(windows))]
        {
            Err(DriverError::DependencyUnavailable(
                "machine owner guard requires Windows".into(),
            ))
        }
    }
}
impl Drop for OwnerGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle as _);
        }
    }
}
#[cfg(windows)]
mod native {
    use super::*;
    use std::{ffi::c_void, mem, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, LocalFree, ERROR_ALREADY_EXISTS},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        },
        System::Threading::{CreateMutexW, GetCurrentProcess, OpenProcessToken},
    };
    fn error(operation: &str) -> DriverError {
        DriverError::Native {
            operation: operation.into(),
            status: unsafe { GetLastError() } as i32,
            transferred: 0,
        }
    }
    fn current_sid() -> DriverResult<String> {
        let mut token = ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(error("OpenProcessToken"));
        }
        let result = (|| {
            let mut size = 0;
            unsafe {
                GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
            }
            if size < mem::size_of::<TOKEN_USER>() as u32 || size > 65536 {
                return Err(DriverError::Protocol("invalid TOKEN_USER size".into()));
            }
            let mut buffer = vec![0usize; (size as usize).div_ceil(mem::size_of::<usize>())];
            let capacity = buffer.len() * mem::size_of::<usize>();
            if unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    capacity as u32,
                    &mut size,
                )
            } == 0
            {
                return Err(error("GetTokenInformation"));
            }
            if size as usize > capacity {
                return Err(DriverError::Protocol("TOKEN_USER length overflow".into()));
            }
            let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
            let mut text = ptr::null_mut();
            if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
                return Err(error("ConvertSidToStringSidW"));
            }
            let mut length = 0;
            unsafe {
                while length < 256 && *text.add(length) != 0 {
                    length += 1;
                }
            }
            let sid = if length >= 256 {
                Err(DriverError::Protocol("oversized SID".into()))
            } else {
                Ok(String::from_utf16_lossy(unsafe {
                    std::slice::from_raw_parts(text, length)
                }))
            };
            unsafe {
                LocalFree(text.cast::<c_void>());
            }
            sid
        })();
        unsafe {
            CloseHandle(token);
        }
        result
    }
    pub(super) fn acquire(name: &str) -> DriverResult<usize> {
        let sid = current_sid()?;
        // Exactly the existing Host's current-user ACL and namespace. Holding the
        // object handle, rather than thread-affine mutex ownership, defines ownership.
        let sddl: Vec<u16> = format!("D:P(A;;0x001f0001;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(error("ConvertStringSecurityDescriptor"));
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let handle = unsafe { CreateMutexW(&attributes, 0, name.as_ptr()) };
        let code = unsafe { GetLastError() };
        unsafe {
            LocalFree(descriptor);
        }
        if handle.is_null() {
            return Err(DriverError::Native {
                operation: "CreateMutexW(owner)".into(),
                status: code as i32,
                transferred: 0,
            });
        }
        if code == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(handle);
            }
            return Err(DriverError::Busy(
                "another Host/diagnostic owns this machine".into(),
            ));
        }
        Ok(handle as usize)
    }
}
