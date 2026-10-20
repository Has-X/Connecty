use windows::{
    Win32::{
        Foundation::VARIANT_BOOL,
        System::{Com::*, TaskScheduler::*, Variant::VARIANT},
    },
    core::{BSTR, Interface},
};

struct Apartment;
impl Apartment {
    fn init() -> windows::core::Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
fn service() -> windows::core::Result<ITaskService> {
    unsafe {
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let empty = VARIANT::default();
        service.Connect(&empty, &empty, &empty, &empty)?;
        Ok(service)
    }
}
fn name() -> std::io::Result<BSTR> {
    Ok(BSTR::from(format!(
        "Connecty Agent {}",
        crate::platform::user_sid()?
    )))
}
pub fn enabled() -> bool {
    let Ok(_apartment) = Apartment::init() else {
        return false;
    };
    unsafe {
        service()
            .and_then(|s| s.GetFolder(&BSTR::from("\\")))
            .and_then(|f| f.GetTask(&name().map_err(|_| windows::core::Error::from_thread())?))
            .and_then(|t| t.Enabled())
            .map(|v| v.as_bool())
            .unwrap_or(false)
    }
}
fn task_profile_dir() -> Option<std::path::PathBuf> {
    let _apartment = Apartment::init().ok()?;
    unsafe {
        let root = service().ok()?.GetFolder(&BSTR::from("\\")).ok()?;
        let task = root.GetTask(&name().ok()?).ok()?;
        let action: IExecAction = task
            .Definition()
            .ok()?
            .Actions()
            .ok()?
            .get_Item(1)
            .ok()?
            .cast()
            .ok()?;
        let mut arguments = BSTR::new();
        action.Arguments(&mut arguments).ok()?;
        let args = arguments.to_string();
        let path = args
            .strip_prefix("--watchdog --profile-dir \"")?
            .strip_suffix('"')?;
        let path = std::path::PathBuf::from(path);
        path.is_absolute().then_some(path)
    }
}
pub fn profile_dir() -> Option<std::path::PathBuf> {
    task_profile_dir().or_else(|| {
        use windows_sys::Win32::System::Registry::*;
        let key: Vec<u16> = "Software\\Chromatic\\Connecty\0".encode_utf16().collect();
        let value: Vec<u16> = "ProfileDirectory\0".encode_utf16().collect();
        let mut data = vec![0u16; 32768];
        let mut size = (data.len() * 2) as u32;
        unsafe {
            if RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                data.as_mut_ptr().cast(),
                &mut size,
            ) != 0
            {
                return None;
            }
        }
        let path = std::path::PathBuf::from(
            String::from_utf16_lossy(&data[..size as usize / 2]).trim_end_matches('\0'),
        );
        path.is_absolute().then_some(path)
    })
}
fn remember_profile_dir(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::*;
    let key: Vec<u16> = "Software\\Chromatic\\Connecty\0".encode_utf16().collect();
    let value: Vec<u16> = "ProfileDirectory\0".encode_utf16().collect();
    let data: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut handle = std::ptr::null_mut();
        let result = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut handle,
            std::ptr::null_mut(),
        );
        if result != 0 {
            return Err(std::io::Error::from_raw_os_error(result as i32));
        }
        let result = RegSetValueExW(
            handle,
            value.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        );
        RegCloseKey(handle);
        if result != 0 {
            return Err(std::io::Error::from_raw_os_error(result as i32));
        }
    }
    Ok(())
}
pub fn executable() -> Option<std::path::PathBuf> {
    let _apartment = Apartment::init().ok()?;
    unsafe {
        let task = service()
            .ok()?
            .GetFolder(&BSTR::from("\\"))
            .ok()?
            .GetTask(&name().ok()?)
            .ok()?;
        let action: IExecAction = task
            .Definition()
            .ok()?
            .Actions()
            .ok()?
            .get_Item(1)
            .ok()?
            .cast()
            .ok()?;
        let mut path = BSTR::new();
        action.Path(&mut path).ok()?;
        Some(std::path::PathBuf::from(path.to_string()))
    }
}
pub fn remove_owned() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !executable().is_some_and(|p| {
        p.to_string_lossy().eq_ignore_ascii_case(
            &std::env::current_exe()
                .unwrap_or_default()
                .to_string_lossy(),
        )
    }) {
        return Ok(());
    }
    let _apartment = Apartment::init()?;
    unsafe {
        service()?
            .GetFolder(&BSTR::from("\\"))?
            .DeleteTask(&name()?, 0)?;
    }
    Ok(())
}
pub fn set(
    enabled: bool,
    data_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    set_at(enabled, data_dir, &std::env::current_exe()?)
}
pub fn set_at(
    enabled: bool,
    data_dir: &std::path::Path,
    exe: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    remember_profile_dir(data_dir)?;
    let _apartment = Apartment::init()?;
    let sid = crate::platform::user_sid()?;
    let name = name()?;
    unsafe {
        let service = service()?;
        let root = service.GetFolder(&BSTR::from("\\"))?;
        if !enabled {
            if let Ok(task) = root.GetTask(&name) {
                // Keep the resolved folder locator when login startup is off.
                task.SetEnabled(VARIANT_BOOL::from(false))?;
            }
            return Ok(());
        }
        let definition = service.NewTask(0)?;
        let principal = definition.Principal()?;
        principal.SetUserId(&BSTR::from(&sid))?;
        principal.SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)?;
        principal.SetRunLevel(TASK_RUNLEVEL_LUA)?;
        let trigger: ILogonTrigger = definition.Triggers()?.Create(TASK_TRIGGER_LOGON)?.cast()?;
        trigger.SetUserId(&BSTR::from(&sid))?;
        let action: IExecAction = definition.Actions()?.Create(TASK_ACTION_EXEC)?.cast()?;
        action.SetPath(&BSTR::from(exe.to_string_lossy().as_ref()))?;
        action.SetArguments(&BSTR::from(format!(
            "--watchdog --profile-dir \"{}\"",
            data_dir.display()
        )))?;
        action.SetWorkingDirectory(&BSTR::from(
            exe.parent().unwrap().to_string_lossy().as_ref(),
        ))?;
        let settings = definition.Settings()?;
        settings.SetExecutionTimeLimit(&BSTR::from("PT0S"))?;
        settings.SetRestartCount(999)?;
        settings.SetRestartInterval(&BSTR::from("PT1M"))?;
        settings.SetMultipleInstances(TASK_INSTANCES_IGNORE_NEW)?;
        settings.SetStartWhenAvailable(VARIANT_BOOL::from(true))?;
        settings.SetDisallowStartIfOnBatteries(VARIANT_BOOL::from(false))?;
        settings.SetStopIfGoingOnBatteries(VARIANT_BOOL::from(false))?;
        root.RegisterTaskDefinition(
            &name,
            &definition,
            TASK_CREATE_OR_UPDATE.0,
            &VARIANT::from(sid.as_str()),
            &VARIANT::default(),
            TASK_LOGON_INTERACTIVE_TOKEN,
            &VARIANT::default(),
        )?;
    }
    Ok(())
}
