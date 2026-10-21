//! Short-lived installer maintenance. Never runs in the transport reactor.
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::os::windows::{ffi::OsStrExt, io::AsRawHandle, process::CommandExt};
use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use windows_sys::Win32::{Foundation::*, System::Threading::*, UI::WindowsAndMessaging::*};

#[derive(Default, Serialize, Deserialize)]
struct InstallState {
    directory: PathBuf,
    startup: bool,
    running: bool,
    executable: PathBuf,
}
fn control(exe: &Path, op: &str) -> io::Result<serde_json::Value> {
    let mut child = Command::new(exe)
        .args(["--ctl", &serde_json::json!({"op":op}).to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Missing maintenance output"))?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        output
            .take(2 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let completed =
        unsafe { WaitForSingleObject(child.as_raw_handle() as HANDLE, 20000) == WAIT_OBJECT_0 };
    if !completed {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let bytes = reader
        .join()
        .map_err(|_| io::Error::other("Maintenance reader failed"))??;
    if !completed || !status.success() {
        return Err(io::Error::other(
            "Agent maintenance command failed or timed out",
        ));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}
fn close_ui(agent: &Path) -> io::Result<()> {
    let title: Vec<u16> = "Connecty\0".encode_utf16().collect();
    unsafe {
        let window = FindWindowW(std::ptr::null(), title.as_ptr());
        if window.is_null() {
            return Ok(());
        }
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        let process = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        );
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut path = [0u16; 32768];
        let mut length = path.len() as u32;
        let same = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) != 0
            && String::from_utf16_lossy(&path[..length as usize])
                .eq_ignore_ascii_case(&agent.with_file_name("Connecty.exe").to_string_lossy());
        if same {
            PostMessageW(window, WM_CLOSE, 0, 0);
            if WaitForSingleObject(process, 10000) != WAIT_OBJECT_0 {
                CloseHandle(process);
                return Err(io::Error::other("Close Connecty before continuing setup"));
            }
        }
        CloseHandle(process);
    }
    Ok(())
}
fn start(state: &InstallState) -> io::Result<()> {
    if state.running {
        crate::startup::run_registered().map_err(|e| io::Error::other(e.to_string()))?;
    }
    Ok(())
}
pub fn run(operation: &str, state_path: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let current = std::env::current_exe()?;
    if operation == "--setup-remove" {
        close_ui(&current)?;
        // Only this installation may stop its pipe owner or remove its task.
        if let Ok(reply) = control(&current, "snapshot")
            && let Some(pid) = reply["snapshot"]["agentPid"].as_u64()
        {
            unsafe {
                let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid as u32);
                let stopped = control(&current, "shutdown");
                let result = if process.is_null() {
                    WAIT_OBJECT_0
                } else {
                    WaitForSingleObject(process, 15000)
                };
                if !process.is_null() {
                    CloseHandle(process);
                }
                stopped?;
                if result != WAIT_OBJECT_0 {
                    return Err("Agent shutdown timed out".into());
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        crate::startup::remove_owned().map_err(|e| io::Error::other(e.to_string()))?;
        return Ok(());
    }
    let state_path = Path::new(state_path.ok_or("Missing setup state file")?);
    if operation == "--setup-rollback" {
        let state: InstallState = serde_json::from_slice(&std::fs::read(state_path)?)?;
        if state.running && state.executable.is_file() {
            crate::startup::set_at(true, &state.directory, &state.executable)
                .map_err(|e| io::Error::other(e.to_string()))?;
            if !state.startup {
                crate::startup::set_at(false, &state.directory, &state.executable)
                    .map_err(|e| io::Error::other(e.to_string()))?;
            }
            crate::startup::run_registered().map_err(|e| io::Error::other(e.to_string()))?;
        }
        return Ok(());
    }
    if operation == "--setup-finish" {
        let state: InstallState = serde_json::from_slice(&std::fs::read(state_path)?)?;
        if !state.directory.is_absolute() {
            return Err("Invalid setup profile directory".into());
        }
        // A fresh installation with login startup off needs only the locator;
        // do not require task registration until the user enables it. Upgrades
        // rewrite an existing task or restart a previously running agent.
        if state.startup || state.running || crate::startup::executable().is_some() {
            crate::startup::set(true, &state.directory)
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        if !state.startup {
            crate::startup::set(false, &state.directory)
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        start(&state)?;
        return Ok(());
    }
    let old = crate::startup::executable()
        .filter(|p| p.is_file())
        .unwrap_or(current);
    let mut state = InstallState {
        directory: crate::startup::profile_dir().unwrap_or(crate::config::data_dir()?),
        startup: crate::startup::enabled(),
        running: false,
        executable: old.clone(),
    };
    if let Ok(reply) = control(&old, "snapshot") {
        let snapshot = &reply["snapshot"];
        if let Some(path) = snapshot["dataDirectory"].as_str() {
            state.directory = PathBuf::from(path);
        }
        state.startup = snapshot["startup"].as_bool().unwrap_or(state.startup);
        state.running = true;
        // Persist recovery information before stopping anything.
        std::fs::write(state_path, serde_json::to_vec(&state)?)?;
        close_ui(&old)?;
        let pid = snapshot["agentPid"]
            .as_u64()
            .ok_or("Invalid agent process")? as u32;
        unsafe {
            let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if let Err(error) = control(&old, "shutdown") {
                if !process.is_null() {
                    CloseHandle(process);
                }
                return Err(error.into());
            }
            if !process.is_null() {
                let result = WaitForSingleObject(process, 15000);
                CloseHandle(process);
                if result != WAIT_OBJECT_0 {
                    return Err("Agent shutdown timed out".into());
                }
            }
        }
        // The launcher's blocking process wait exits immediately after its child.
        std::thread::sleep(Duration::from_millis(200));
    } else {
        let name = crate::platform::pipe_name(None)?;
        let wide: Vec<u16> = std::ffi::OsStr::new(&name)
            .encode_wide()
            .chain(Some(0))
            .collect();
        unsafe {
            if windows_sys::Win32::System::Pipes::WaitNamedPipeW(wide.as_ptr(), 1) != 0
                || !matches!(GetLastError(), ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND)
            {
                return Err("Another Connecty copy is running. Close it before installing.".into());
            }
        }
    }
    std::fs::write(state_path, serde_json::to_vec(&state)?)?;
    Ok(())
}
