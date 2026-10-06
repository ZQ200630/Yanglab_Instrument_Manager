use super::contracts::HostError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    ffi::c_void,
    os::windows::io::{AsRawHandle, RawHandle},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeClient, NamedPipeServer, ServerOptions};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, HANDLE, INVALID_HANDLE_VALUE},
    Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    },
    Security::{GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER},
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, SECURITY_IDENTIFICATION,
        SECURITY_SQOS_PRESENT,
    },
    System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
    System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    },
};

pub const MAX_FRAME: usize = 65_536;
pub const MAX_CLIENTS: usize = 16;
pub const MAX_CHANNELS: usize = MAX_CLIENTS * 5;
pub const CLIENT_MASK: u32 = 0x0012_0183; // DATA read/write, attributes, READ_CONTROL, SYNCHRONIZE; no bit 4.
fn io_error(error: std::io::Error) -> HostError {
    HostError::new("LocalIpc", error.to_string())
}
pub(crate) fn wide(value: &str) -> Result<Vec<u16>, HostError> {
    if value.contains('\0') {
        return Err(HostError::new("LocalIpc", "NUL in Windows name"));
    }
    Ok(value.encode_utf16().chain(Some(0)).collect())
}
pub(crate) struct SecurityDescriptor {
    pointer: *mut c_void,
}
impl SecurityDescriptor {
    pub fn new(sddl: &str) -> Result<Self, HostError> {
        let text = wide(sddl)?;
        let mut pointer = std::ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut pointer,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        Ok(Self { pointer })
    }
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.pointer,
            bInheritHandle: 0,
        }
    }
}
impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.pointer) };
    }
}
fn process_sid(handle: HANDLE) -> Result<String, HostError> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(handle, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    let result = (|| {
        let mut size = 0;
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size) };
        if size == 0 || size > 65536 {
            return Err(HostError::new("LocalIpc", "Invalid token size"));
        }
        // usize storage supplies TOKEN_USER alignment.
        let mut buffer = vec![
            0usize;
            (size as usize + std::mem::size_of::<usize>() - 1)
                / std::mem::size_of::<usize>()
        ];
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr() as *mut c_void,
                size,
                &mut size,
            )
        } == 0
        {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        let mut text = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        let mut length = 0;
        unsafe {
            while length < 256 && *text.add(length) != 0 {
                length += 1;
            }
        }
        let value = if length >= 256 {
            Err(HostError::new("LocalIpc", "Invalid SID"))
        } else {
            Ok(String::from_utf16_lossy(unsafe {
                std::slice::from_raw_parts(text, length)
            }))
        };
        unsafe { LocalFree(text as *mut c_void) };
        value
    })();
    unsafe { CloseHandle(token) };
    result
}
#[derive(Clone)]
pub struct PipeSecurity {
    pub sid: String,
    pub sddl: String,
    pub client_mask: u32,
    pub reject_remote: bool,
}
impl PipeSecurity {
    pub fn current() -> Result<Self, HostError> {
        let sid = process_sid(unsafe { GetCurrentProcess() })?;
        // Duplex server handles and subsequent instances need bit 4. Use explicit
        // rights, not generic-write; clients request CLIENT_MASK without bit 4.
        let sddl = format!("O:{sid}G:{sid}D:P(A;;0x0012019f;;;{sid})");
        Ok(Self {
            sid,
            sddl,
            client_mask: CLIENT_MASK,
            reject_remote: true,
        })
    }
    pub fn accepts_sid(&self, sid: &str, remote: bool) -> bool {
        !remote && sid == self.sid
    }
    pub fn endpoint(&self) -> String {
        format!("\\\\.\\pipe\\YangLab-{}", self.sid)
    }
    pub fn create_server(&self, endpoint: &str, first: bool) -> Result<NamedPipeServer, HostError> {
        validate_endpoint(endpoint, &self.sid)?;
        let descriptor = SecurityDescriptor::new(&self.sddl)?;
        let attributes = descriptor.attributes();
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .max_instances(MAX_CHANNELS + 2)
                .create_with_security_attributes_raw(
                    endpoint,
                    &attributes as *const _ as *mut c_void,
                )
        }
        .map_err(io_error)
    }
    pub fn verify_client(&self, pipe: &NamedPipeServer) -> Result<(), HostError> {
        let mut pid = 0;
        if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle() as HANDLE, &mut pid) } == 0 {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        self.verify_pid(pid)
    }
    fn verify_pid(&self, pid: u32) -> Result<(), HostError> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        let sid = process_sid(process);
        unsafe { CloseHandle(process) };
        if sid? != self.sid {
            return Err(HostError::new(
                "UntrustedClient",
                "Peer Windows user differs",
            ));
        }
        Ok(())
    }
    pub fn open_client(&self, endpoint: &str) -> Result<NamedPipeClient, HostError> {
        validate_endpoint(endpoint, &self.sid)?;
        let name = wide(endpoint)?;
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                CLIENT_MASK,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let error = std::io::Error::last_os_error();
            return Err(if error.raw_os_error() == Some(2) {
                HostError::new("HostAbsent", "Local Host is not running")
            } else {
                io_error(error)
            });
        }
        let mut pid = 0;
        if unsafe { GetNamedPipeServerProcessId(handle, &mut pid) } == 0 {
            let error = io_error(std::io::Error::last_os_error());
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        if let Err(error) = self.verify_pid(pid) {
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        // Tokio owns the handle even if reactor registration fails.
        unsafe { NamedPipeClient::from_raw_handle(handle as RawHandle) }.map_err(io_error)
    }
}
fn validate_endpoint(endpoint: &str, sid: &str) -> Result<(), HostError> {
    let base = format!("\\\\.\\pipe\\YangLab-{sid}");
    if endpoint != base
        && !(endpoint.starts_with(&(base + "-test-"))
            && endpoint
                .rsplit('-')
                .next()
                .is_some_and(super::contracts::valid_id))
    {
        return Err(HostError::new(
            "LocalIpc",
            "Only the current-user local Host namespace is allowed",
        ));
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRequest {
    pub v: u64,
    pub id: String,
    pub method: String,
    pub params: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostReply {
    pub v: u64,
    pub id: String,
    pub ok: bool,
    pub result: Value,
    pub error: Option<HostError>,
}
impl HostReply {
    pub fn from_result(id: String, result: Result<Value, HostError>) -> Self {
        match result {
            Ok(result) => Self {
                v: 1,
                id,
                ok: true,
                result,
                error: None,
            },
            Err(error) => Self {
                v: 1,
                id,
                ok: false,
                result: Value::Null,
                error: Some(error),
            },
        }
    }
}
pub fn parse_request(bytes: &[u8]) -> Result<HostRequest, HostError> {
    if bytes.len() > MAX_FRAME {
        return Err(HostError::new(
            "FrameCapacity",
            "Host request exceeds 64 KiB",
        ));
    }
    let value = crate::runtime::strict_json(bytes)
        .map_err(|error| HostError::new("HostProtocol", error))?;
    let request: HostRequest = serde_json::from_value(value)
        .map_err(|error| HostError::new("HostProtocol", error.to_string()))?;
    if request.v != 1
        || request.id.is_empty()
        || request.id.len() > 64
        || request.id.chars().any(char::is_control)
        || !request.params.is_object()
        || request.method.is_empty()
        || request.method.len() > 64
    {
        return Err(HostError::new(
            "HostProtocol",
            "Invalid Host version, ID, method or parameters",
        ));
    }
    Ok(request)
}
pub async fn read_frame<R: AsyncRead + Unpin>(
    source: &mut R,
) -> Result<Option<Vec<u8>>, HostError> {
    read_frame_limit(source, MAX_FRAME).await
}
pub async fn read_frame_limit<R: AsyncRead + Unpin>(
    source: &mut R,
    limit: usize,
) -> Result<Option<Vec<u8>>, HostError> {
    let mut prefix = [0u8; 4];
    match source.read(&mut prefix[..1]).await {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(error) => return Err(io_error(error)),
    }
    source
        .read_exact(&mut prefix[1..])
        .await
        .map_err(|_| HostError::new("TruncatedFrame", "Incomplete frame prefix"))?;
    let length = u32::from_le_bytes(prefix) as usize;
    if length == 0 || length > limit {
        return Err(HostError::new(
            "FrameCapacity",
            "Frame exceeds 64 KiB or is empty",
        ));
    }
    let mut bytes = vec![0; length];
    source
        .read_exact(&mut bytes)
        .await
        .map_err(|_| HostError::new("TruncatedFrame", "Incomplete frame body"))?;
    Ok(Some(bytes))
}
pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    target: &mut W,
    value: &T,
) -> Result<(), HostError> {
    write_frame_limit(target, value, MAX_FRAME).await
}
pub async fn write_frame_limit<W: AsyncWrite + Unpin, T: Serialize>(
    target: &mut W,
    value: &T,
    limit: usize,
) -> Result<(), HostError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| HostError::new("HostProtocol", error.to_string()))?;
    if bytes.len() > limit {
        return Err(HostError::new("FrameCapacity", "Reply exceeds 64 KiB"));
    }
    target
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .await
        .map_err(io_error)?;
    target.write_all(&bytes).await.map_err(io_error)?;
    target.flush().await.map_err(io_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_local_pipe_is_distinct_from_untrusted_or_broken_host() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let security = PipeSecurity::current().unwrap();
            let endpoint = format!(
                "{}-test-{}",
                security.endpoint(),
                crate::host::registry::new_id().unwrap()
            );
            let error = match security.open_client(&endpoint) {
                Err(e) => e,
                Ok(_) => panic!("unexpected existing Host"),
            };
            assert_eq!(error.code, "HostAbsent");
        });
    }
    #[test]
    fn pipe_rejects_untrusted_and_remote_clients() {
        let security = PipeSecurity::current().unwrap();
        assert_eq!(security.client_mask & 4, 0);
        assert!(security.reject_remote);
        assert!(!security.accepts_sid("S-1-5-21-0-0-0-1000", false));
        assert!(!security.accepts_sid(&security.sid, true));
        assert!(security.accepts_sid(&security.sid, false));
        assert!(!security.sddl.contains("WD)"));
    }
    #[test]
    fn oversized_truncated_and_duplicate_frames_never_reach_dispatch() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut input = std::io::Cursor::new((65537u32).to_le_bytes().to_vec());
            assert_eq!(
                read_frame(&mut input).await.unwrap_err().code,
                "FrameCapacity"
            );
            let mut bytes = 9u32.to_le_bytes().to_vec();
            bytes.extend_from_slice(b"{}");
            assert_eq!(
                read_frame(&mut std::io::Cursor::new(bytes))
                    .await
                    .unwrap_err()
                    .code,
                "TruncatedFrame"
            );
            assert!(
                parse_request(br#"{"v":1,"v":1,"id":"a","method":"ping","params":{}}"#).is_err()
            );
        });
    }
    #[test]
    fn windows_access_check_denies_another_sid() {
        use windows_sys::Win32::Foundation::LUID;
        use windows_sys::Win32::Security::Authorization::*;
        let security = PipeSecurity::current().unwrap();
        let descriptor = SecurityDescriptor::new(&security.sddl).unwrap();
        for (sid, allowed) in [
            (&security.sid, true),
            (&"S-1-5-21-1-2-3-1234".to_string(), false),
        ] {
            let text = wide(sid).unwrap();
            let mut user = std::ptr::null_mut();
            let mut manager = std::ptr::null_mut();
            let mut context = std::ptr::null_mut();
            unsafe {
                assert_ne!(ConvertStringSidToSidW(text.as_ptr(), &mut user), 0);
                assert_ne!(
                    AuthzInitializeResourceManager(
                        AUTHZ_RM_FLAG_NO_AUDIT,
                        None,
                        None,
                        None,
                        std::ptr::null(),
                        &mut manager
                    ),
                    0
                );
                let initialized = AuthzInitializeContextFromSid(
                    AUTHZ_SKIP_TOKEN_GROUPS,
                    user,
                    manager,
                    std::ptr::null(),
                    LUID {
                        LowPart: 0,
                        HighPart: 0,
                    },
                    std::ptr::null(),
                    &mut context,
                );
                if initialized == 0 {
                    AuthzFreeResourceManager(manager);
                    LocalFree(user);
                    panic!("Authz context: {}", std::io::Error::last_os_error())
                }
                let request = AUTHZ_ACCESS_REQUEST {
                    DesiredAccess: CLIENT_MASK,
                    ..Default::default()
                };
                let mut granted = 0;
                let mut error = 0;
                let mut reply = AUTHZ_ACCESS_REPLY {
                    ResultListLength: 1,
                    GrantedAccessMask: &mut granted,
                    Error: &mut error,
                    ..Default::default()
                };
                let checked = AuthzAccessCheck(
                    0,
                    context,
                    &request,
                    std::ptr::null_mut(),
                    descriptor.pointer,
                    std::ptr::null(),
                    0,
                    &mut reply,
                    std::ptr::null_mut(),
                );
                AuthzFreeContext(context);
                AuthzFreeResourceManager(manager);
                LocalFree(user);
                assert_ne!(
                    checked,
                    0,
                    "AuthzAccessCheck failed: {}",
                    std::io::Error::last_os_error()
                );
                assert_eq!(error == 0 && granted == CLIENT_MASK, allowed);
            }
        }
    }
}
