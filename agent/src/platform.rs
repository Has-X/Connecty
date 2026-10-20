use crate::ssh::Recovery;
use std::os::windows::ffi::OsStrExt;
use std::{
    ffi::c_void,
    io,
    path::Path,
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicIsize, Ordering},
    },
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use windows_sys::Win32::{
    Foundation::*,
    NetworkManagement::IpHelper::*,
    Networking::WinSock::AF_UNSPEC,
    Security::Authorization::*,
    Security::*,
    System::{LibraryLoader::*, Threading::*},
    UI::{Shell::*, WindowsAndMessaging::*},
};
fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}
fn last() -> io::Error {
    io::Error::last_os_error()
}
fn sid_for(process: HANDLE) -> io::Result<String> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return Err(last());
        }
        let mut size = 0;
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
        let mut buffer = vec![0u8; size as usize];
        if GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            CloseHandle(token);
            return Err(last());
        }
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut text = ptr::null_mut();
        let ok = ConvertSidToStringSidW(user.User.Sid, &mut text);
        CloseHandle(token);
        if ok == 0 {
            return Err(last());
        }
        let mut n = 0;
        while *text.add(n) != 0 {
            n += 1;
        }
        let result = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text.cast());
        Ok(result)
    }
}
pub fn user_sid() -> io::Result<String> {
    sid_for(unsafe { GetCurrentProcess() })
}
pub struct Security {
    descriptor: PSECURITY_DESCRIPTOR,
    attrs: Box<SECURITY_ATTRIBUTES>,
}
unsafe impl Send for Security {}
impl Security {
    pub fn new() -> io::Result<Self> {
        unsafe {
            let sid = user_sid()?;
            let sddl = wide(format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"));
            let mut descriptor = ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            ) == 0
            {
                return Err(last());
            }
            let attrs = Box::new(SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            });
            Ok(Self { descriptor, attrs })
        }
    }
    pub fn attributes(&self) -> *const SECURITY_ATTRIBUTES {
        &*self.attrs
    }
}
impl Drop for Security {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.descriptor);
        }
    }
}
pub fn pipe_name(custom_data: Option<&Path>) -> io::Result<String> {
    let mut name = format!(r"\\.\pipe\Connecty.Agent.{}.v1", user_sid()?);
    if let Some(path) = custom_data {
        let text = path.to_string_lossy().to_lowercase();
        let hash = text.as_bytes().iter().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        });
        name += &format!(".{hash:016x}");
    }
    Ok(name)
}
pub fn same_user_client(pipe: &tokio::net::windows::named_pipe::NamedPipeServer) -> bool {
    use std::os::windows::io::AsRawHandle;
    unsafe {
        let mut pid = 0;
        if windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId(
            pipe.as_raw_handle(),
            &mut pid,
        ) == 0
        {
            return false;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let sid = sid_for(process);
        CloseHandle(process);
        match (sid, user_sid()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}
pub fn trusted_server(pipe: &tokio::net::windows::named_pipe::NamedPipeClient) -> bool {
    use std::os::windows::io::AsRawHandle;
    unsafe {
        let mut pid = 0;
        if windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId(
            pipe.as_raw_handle(),
            &mut pid,
        ) == 0
        {
            return false;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut path = vec![0u16; 32768];
        let mut size = path.len() as u32;
        let queried = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut size) != 0;
        let sid = sid_for(process);
        CloseHandle(process);
        queried
            && matches!((sid, user_sid(), std::env::current_exe()), (Ok(a), Ok(b), Ok(exe)) if a == b && String::from_utf16_lossy(&path[..size as usize]).eq_ignore_ascii_case(&exe.to_string_lossy()))
    }
}
struct Native {
    recovery: watch::Sender<Recovery>,
    shutdown: CancellationToken,
    hwnd: AtomicIsize,
    ui: std::path::PathBuf,
    summary: std::sync::Mutex<(String, String)>,
    show_tray: bool,
    thread: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}
static NATIVE: OnceLock<Native> = OnceLock::new();
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
pub fn inject_recovery(event: Recovery) {
    if let Some(native) = NATIVE.get() {
        if event == Recovery::Network && *native.recovery.borrow() == Recovery::Suspend {
            return;
        }
        if event == Recovery::Resume && *native.recovery.borrow() == Recovery::Resume {
            return;
        }
        native.recovery.send_replace(event);
    }
}
pub fn update_tray(snapshot: &crate::model::Snapshot) {
    if let Some(native) = NATIVE.get() {
        let connected = snapshot
            .connections
            .values()
            .filter(|s| s.state == "forwarding" || s.state == "verified")
            .count();
        let retry = snapshot
            .connections
            .values()
            .any(|s| s.state == "connecting" || s.state == "retrying");
        let error =
            snapshot.error.is_some() || snapshot.connections.values().any(|s| s.state == "error");
        let suffix = if retry {
            " (reconnecting)"
        } else if error {
            " (needs attention)"
        } else {
            ""
        };
        *native.summary.lock().unwrap_or_else(|p| p.into_inner()) = (
            format!("Connecty: {connected} connected{suffix}"),
            if retry {
                "Connecty-retry.ico"
            } else if error {
                "Connecty-error.ico"
            } else if connected > 0 {
                "Connecty-live.ico"
            } else {
                "Connecty-tray.ico"
            }
            .into(),
        );
        let hwnd = native.hwnd.load(Ordering::Relaxed);
        if hwnd != 0 {
            unsafe {
                PostMessageW(hwnd as HWND, WM_APP + 1, 0, 0);
            }
        }
    }
}
fn tray(hwnd: HWND, op: u32) {
    let native = NATIVE.get().unwrap();
    if !native.show_tray {
        return;
    }
    unsafe {
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of_val(&data) as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_APP + 2;
        let (summary, icon) = native
            .summary
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let path = native.ui.with_file_name(icon);
        data.hIcon = LoadImageW(
            ptr::null_mut(),
            wide(path).as_ptr(),
            IMAGE_ICON,
            32,
            32,
            LR_LOADFROMFILE,
        ) as HICON;
        let tip = wide(summary);
        data.szTip[..tip.len()].copy_from_slice(&tip);
        Shell_NotifyIconW(op, &data);
        if !data.hIcon.is_null() {
            DestroyIcon(data.hIcon);
        }
        if op == NIM_ADD {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
    }
}
unsafe extern "system" fn address_changed(
    _context: *const c_void,
    _row: *const MIB_UNICASTIPADDRESS_ROW,
    _kind: MIB_NOTIFICATION_TYPE,
) {
    inject_recovery(Recovery::Network);
}
unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_POWERBROADCAST {
            match w as u32 {
                PBT_APMSUSPEND => inject_recovery(Recovery::Suspend),
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => inject_recovery(Recovery::Resume),
                _ => {}
            }
            return 1;
        }
        if message == WM_APP + 1 {
            tray(hwnd, NIM_MODIFY);
            return 0;
        }
        if message == WM_APP + 2 {
            let event = (l as u32) & 0xffff;
            if (event == WM_CONTEXTMENU || event == NIN_SELECT || event == NIN_KEYSELECT)
                && let Some(native) = NATIVE.get()
            {
                use std::os::windows::process::CommandExt;
                let mut command = std::process::Command::new(&native.ui);
                if event == WM_CONTEXTMENU {
                    command.arg("--tray-popup");
                }
                if let Ok(child) = command.creation_flags(CREATE_NO_WINDOW).spawn() {
                    AllowSetForegroundWindow(child.id());
                }
            }
            return 0;
        }
        if message == WM_QUERYENDSESSION {
            return 1;
        }
        if message == WM_ENDSESSION && w != 0 {
            if let Some(native) = NATIVE.get() {
                native.shutdown.cancel();
            }
            return 0;
        }
        if message == WM_DESTROY {
            tray(hwnd, NIM_DELETE);
            PostQuitMessage(0);
            return 0;
        }
        let taskbar = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        if message == taskbar {
            tray(hwnd, NIM_ADD);
            return 0;
        }
        DefWindowProcW(hwnd, message, w, l)
    }
}
pub fn start(
    recovery: watch::Sender<Recovery>,
    shutdown: CancellationToken,
    show_tray: bool,
) -> io::Result<()> {
    let ui = std::env::current_exe()?.with_file_name("Connecty.exe");
    NATIVE
        .set(Native {
            recovery,
            shutdown,
            hwnd: AtomicIsize::new(0),
            ui,
            summary: std::sync::Mutex::new((
                "Connecty · background agent".into(),
                "Connecty-tray.ico".into(),
            )),
            show_tray,
            thread: std::sync::Mutex::new(None),
        })
        .map_err(|_| io::Error::other("Platform already initialized"))?;
    let (ready, started) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("connecty-windows".into())
        .spawn(move || unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let class = wide("Connecty.Agent.Events");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(window_proc);
            wc.hInstance = instance;
            wc.lpszClassName = class.as_ptr();
            if RegisterClassW(&wc) == 0 {
                let _ = ready.send(Err(last()));
                return;
            }
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                wide("Connecty background agent").as_ptr(),
                0,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
            if hwnd.is_null() {
                let _ = ready.send(Err(last()));
                return;
            }
            NATIVE
                .get()
                .unwrap()
                .hwnd
                .store(hwnd as isize, Ordering::Relaxed);
            if show_tray {
                tray(hwnd, NIM_ADD);
            }
            let mut notification = ptr::null_mut();
            let result = NotifyUnicastIpAddressChange(
                AF_UNSPEC,
                Some(address_changed),
                ptr::null(),
                false,
                &mut notification,
            );
            if result != 0 {
                let _ = ready.send(Err(io::Error::from_raw_os_error(result as i32)));
                DestroyWindow(hwnd);
                return;
            }
            let _ = ready.send(Ok(()));
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if !notification.is_null() {
                CancelMibChangeNotify2(notification);
            }
        })?;
    *NATIVE
        .get()
        .unwrap()
        .thread
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(thread);
    started
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "Windows event thread did not start",
            )
        })?
}
pub fn stop() {
    if let Some(native) = NATIVE.get() {
        let hwnd = native.hwnd.load(Ordering::Relaxed);
        if hwnd != 0 {
            unsafe {
                PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0);
            }
        }
        let thread = native
            .thread
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}
