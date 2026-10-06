//! Native tray owner: no service, autostart, network listener or hidden orphan.
use super::contracts::HostError;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    UI::{Shell::*, WindowsAndMessaging::*},
};
#[derive(Clone)]
pub struct HostStatus {
    pub mode: String,
    pub state: String,
}
pub struct ReopenTarget {
    pub executable: PathBuf,
}
pub struct HostTray {
    window: isize,
    pub status: Arc<Mutex<HostStatus>>,
    pub stop: Arc<AtomicBool>,
}
struct Context {
    status: Arc<Mutex<HostStatus>>,
    stop: Arc<AtomicBool>,
    executable: PathBuf,
    data: NOTIFYICONDATAW,
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn update(ctx: &mut Context) {
    let state = ctx.status.lock().unwrap().clone();
    let tip = wide(&format!(
        "Yang LAB Host · {} · {}",
        state.mode.to_uppercase(),
        state.state
    ));
    ctx.data.szTip.fill(0);
    let n = tip.len().min(ctx.data.szTip.len() - 1);
    ctx.data.szTip[..n].copy_from_slice(&tip[..n]);
    Shell_NotifyIconW(NIM_MODIFY, &ctx.data);
}
unsafe fn reopen(ctx: &Context) {
    use std::os::windows::process::CommandExt;
    if let Err(error) = std::process::Command::new(&ctx.executable)
        .creation_flags(0x08000000)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        MessageBoxW(
            ctx.data.hWnd,
            wide(&format!(
                "Console cannot open: {error}. Host continues running."
            ))
            .as_ptr(),
            wide("Yang LAB Host").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
unsafe extern "system" fn procedure(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let ptr = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut Context;
    if !ptr.is_null() {
        let ctx = &mut *ptr;
        match message {
            WM_TIMER => {
                update(ctx);
                return 0;
            }
            m if m == WM_APP + 1 => {
                if l as u32 == WM_LBUTTONDBLCLK {
                    reopen(ctx);
                    return 0;
                }
                if l as u32 == WM_RBUTTONUP {
                    let menu = CreatePopupMenu();
                    if menu.is_null() {
                        return 0;
                    }
                    let status = ctx.status.lock().unwrap().clone();
                    AppendMenuW(
                        menu,
                        MF_STRING | MF_DISABLED,
                        0,
                        wide(&format!(
                            "{} · {}",
                            status.mode.to_uppercase(),
                            status.state
                        ))
                        .as_ptr(),
                    );
                    AppendMenuW(menu, MF_STRING, 1, wide("Open Console").as_ptr());
                    AppendMenuW(menu, MF_STRING, 2, wide("Stop Host safely…").as_ptr());
                    let mut point = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut point);
                    SetForegroundWindow(window);
                    let chosen = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_NONOTIFY,
                        point.x,
                        point.y,
                        0,
                        window,
                        std::ptr::null(),
                    );
                    DestroyMenu(menu);
                    if chosen == 1 {
                        reopen(ctx)
                    }
                    if chosen==2 && MessageBoxW(window,wide("Stop this Host and revoke all controllers? Voltage: zero. Gain: current off before TEC off. Piezo: hold. A failed stop retains this Host and its management entry.").as_ptr(),wide("Stop Yang LAB Host").as_ptr(),MB_YESNO|MB_ICONWARNING)==IDYES {ctx.stop.store(true,Ordering::Release);}
                    return 0;
                }
            }
            WM_CLOSE => {
                DestroyWindow(window);
                return 0;
            }
            WM_DESTROY => {
                Shell_NotifyIconW(NIM_DELETE, &ctx.data);
                KillTimer(window, 1);
                PostQuitMessage(0);
                return 0;
            }
            _ => {}
        }
    }
    DefWindowProcW(window, message, w, l)
}
impl HostTray {
    pub fn install(
        host_id: &str,
        status: HostStatus,
        reopen: ReopenTarget,
    ) -> Result<Self, HostError> {
        if !super::contracts::valid_id(host_id) || !reopen.executable.is_file() {
            return Err(HostError::new(
                "HostTray",
                "Management executable is missing",
            ));
        }
        let status = Arc::new(Mutex::new(status));
        let stop = Arc::new(AtomicBool::new(false));
        let s = status.clone();
        let t = stop.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("Yang LAB Host tray".into())
            .spawn(move || unsafe {
                let window = CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    wide("Yang LAB Host").as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                if window.is_null() {
                    let _ = tx.send(Err("Native tray window failed".to_string()));
                    return;
                }
                let mut data: NOTIFYICONDATAW = std::mem::zeroed();
                data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
                data.hWnd = window;
                data.uID = 1;
                data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
                data.uCallbackMessage = WM_APP + 1;
                data.hIcon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
                let mut context = Box::new(Context {
                    status: s,
                    stop: t,
                    executable: reopen.executable,
                    data,
                });
                let text = wide("Yang LAB Host · STARTING");
                context.data.szTip[..text.len()].copy_from_slice(&text);
                SetWindowLongPtrW(
                    window,
                    GWLP_USERDATA,
                    (&mut *context as *mut Context) as isize,
                );
                SetWindowLongPtrW(window, GWLP_WNDPROC, procedure as *const () as isize);
                if Shell_NotifyIconW(NIM_ADD, &context.data) == 0 {
                    DestroyWindow(window);
                    let _ = tx.send(Err("Windows notification icon failed".into()));
                    return;
                }
                SetTimer(window, 1, 2500, None);
                let _ = tx.send(Ok(window as isize));
                let mut message: MSG = std::mem::zeroed();
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            })
            .map_err(|e| HostError::new("HostTray", e.to_string()))?;
        let window = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| HostError::new("HostTray", e.to_string()))?
            .map_err(|e| HostError::new("HostTray", e))?;
        Ok(Self {
            window,
            status,
            stop,
        })
    }
}
impl Drop for HostTray {
    fn drop(&mut self) {
        unsafe {
            PostMessageW(self.window as HWND, WM_CLOSE, 0, 0);
        }
    }
}
