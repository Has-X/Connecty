//! A native process wait protects the transport reactor from unexpected crashes.
//! The same binary runs both roles; no runtime or extra executable is shipped.
use std::os::windows::{ffi::OsStrExt, process::CommandExt};
use std::{
    io,
    process::Command,
    time::{Duration, Instant},
};
use windows_sys::Win32::{Foundation::*, System::Threading::*};

struct MutexHandle(HANDLE);
impl Drop for MutexHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    let custom = args
        .iter()
        .position(|a| a == "--data-dir")
        .and_then(|i| args.get(i + 1));
    let pipe = crate::platform::pipe_name(custom.map(std::path::Path::new))?;
    let name: Vec<u16> =
        std::ffi::OsStr::new(&format!("Local\\{}.Launcher", pipe.replace('\\', "_")))
            .encode_wide()
            .chain(Some(0))
            .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let _mutex = MutexHandle(handle);
    if exists {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let mut attempt = 0;
    loop {
        let mut command = Command::new(&exe);
        command.arg("--agent").creation_flags(CREATE_NO_WINDOW);
        if let Some(path) = custom {
            command.arg("--data-dir").arg(path);
        }
        if let Some(i) = args.iter().position(|a| a == "--profile-dir")
            && let Some(path) = args.get(i + 1)
        {
            command.arg("--profile-dir").arg(path);
        }
        for flag in ["--no-tray", "--test-mode"] {
            if args.iter().any(|a| a == flag) {
                command.arg(flag);
            }
        }
        let began = Instant::now();
        let result = command.spawn().and_then(|mut child| child.wait());
        if matches!(result, Ok(status) if status.success()) {
            return Ok(());
        }
        if began.elapsed() > Duration::from_secs(90) {
            attempt = 0;
        }
        // This timer exists only after a failed child, never during normal idle.
        std::thread::sleep(crate::ssh::backoff(attempt, fastrand::f64()));
        attempt = attempt.saturating_add(1);
    }
}
