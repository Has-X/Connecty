use crate::model::{AgentError, Profile, validate_profiles};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Resolved {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity: PathBuf,
    pub known_hosts: PathBuf,
    pub key_host: String,
}
fn known_folder(id: &windows_sys::core::GUID) -> std::io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    unsafe {
        let mut path = std::ptr::null_mut();
        let result = windows_sys::Win32::UI::Shell::SHGetKnownFolderPath(
            id,
            0,
            std::ptr::null_mut(),
            &mut path,
        );
        if result < 0 {
            return Err(std::io::Error::other(format!(
                "Windows folder lookup failed: {result:#x}"
            )));
        }
        let mut len = 0;
        while *path.add(len) != 0 {
            len += 1;
        }
        let value = std::ffi::OsString::from_wide(std::slice::from_raw_parts(path, len));
        windows_sys::Win32::System::Com::CoTaskMemFree(path.cast());
        Ok(PathBuf::from(value))
    }
}
pub fn home() -> PathBuf {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Profile).unwrap_or_default()
}
pub fn data_dir() -> std::io::Result<PathBuf> {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_LocalAppData).map(|p| p.join("Connecty"))
}
fn expand(value: &str) -> PathBuf {
    let h = home().to_string_lossy().to_string();
    PathBuf::from(
        value
            .trim_matches('"')
            .replace("%d", &h)
            .replace("~/", &(h.clone() + "/")),
    )
    .components()
    .collect()
}
fn wildcard(pattern: &str, host: &str) -> bool {
    let (p, s) = (
        pattern.to_ascii_lowercase().into_bytes(),
        host.to_ascii_lowercase().into_bytes(),
    );
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < s.len() {
        if i < p.len() && (p[i] == b'?' || p[i] == s[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            i += 1;
            mark = j;
        } else if let Some(k) = star {
            i = k + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}
fn parse(text: &str, host: &str) -> Result<HashMap<String, String>, AgentError> {
    let mut selected = true;
    let mut out = HashMap::new();
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let Some(pos) = raw.find(|c: char| c.is_whitespace() || c == '=') else {
            continue;
        };
        let key = raw[..pos].to_ascii_lowercase();
        let value = raw[pos..]
            .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
            .trim()
            .trim_matches('"');
        if key == "host" {
            let patterns: Vec<_> = value.split_whitespace().collect();
            selected = patterns
                .iter()
                .any(|p| !p.starts_with('!') && wildcard(p, host))
                && !patterns
                    .iter()
                    .any(|p| p.starts_with('!') && wildcard(&p[1..], host));
            continue;
        }
        if key == "match" || key == "include" {
            return Err(AgentError::config(
                "SSH Include/Match rules need explicit host, user and key fields in Connecty",
            ));
        }
        if !selected {
            continue;
        }
        if (key == "proxycommand" || key == "proxyjump") && value != "none" {
            return Err(AgentError::config(
                "ProxyCommand and ProxyJump are not supported by the embedded client",
            ));
        }
        out.entry(key).or_insert_with(|| value.to_string());
    }
    Ok(out)
}
pub fn resolve(p: &Profile) -> Result<Resolved, AgentError> {
    let (inline_user, alias) = p
        .host
        .rsplit_once('@')
        .map(|(u, h)| (Some(u), h))
        .unwrap_or((None, p.host.as_str()));
    let text = std::fs::read_to_string(home().join(".ssh/config")).unwrap_or_default();
    let values = if !p.hostname.is_empty() && !p.user.is_empty() && !p.identity_file.is_empty() {
        HashMap::new()
    } else {
        parse(&text, alias)?
    };
    let host = if !p.hostname.is_empty() {
        p.hostname.clone()
    } else {
        values
            .get("hostname")
            .cloned()
            .unwrap_or_else(|| alias.into())
    };
    let user = if !p.user.is_empty() {
        p.user.clone()
    } else if let Some(u) = inline_user {
        u.into()
    } else {
        values.get("user").cloned().ok_or_else(|| {
            AgentError::config("Set an SSH username or a User entry in SSH config")
        })?
    };
    let port = if p.port != 0 {
        p.port
    } else {
        values
            .get("port")
            .map(|s| s.parse::<u16>())
            .transpose()
            .map_err(|_| AgentError::config("Invalid SSH port"))?
            .unwrap_or(22)
    };
    let identity = if !p.identity_file.is_empty() {
        expand(&p.identity_file)
    } else if let Some(path) = values.get("identityfile") {
        expand(path)
    } else {
        ["id_ed25519", "id_ecdsa"]
            .into_iter()
            .map(|n| home().join(".ssh").join(n))
            .find(|p| p.is_file())
            .ok_or_else(|| AgentError::config("Choose a private key file"))?
    };
    let known_hosts = if !p.known_hosts.is_empty() {
        expand(&p.known_hosts)
    } else {
        values
            .get("userknownhostsfile")
            .map(|v| expand(v))
            .unwrap_or_else(|| home().join(".ssh/known_hosts"))
    };
    let key_host = values
        .get("hostkeyalias")
        .cloned()
        .unwrap_or_else(|| host.clone());
    if host.contains(['\r', '\n', '\0', '%']) || user.is_empty() || port == 0 {
        return Err(AgentError::config("Invalid resolved SSH endpoint"));
    }
    Ok(Resolved {
        host,
        user,
        port,
        identity,
        known_hosts,
        key_host,
    })
}
pub fn load(path: &Path) -> Result<Vec<Profile>, AgentError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => {
            return Err(AgentError::config(format!(
                "Cannot open saved profiles: {error}"
            )));
        }
    };
    let profiles: Vec<Profile> =
        serde_json::from_slice(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
            .map_err(|e| AgentError::config(format!("Cannot read saved profiles: {e}")))?;
    validate_profiles(&profiles)?;
    Ok(profiles)
}
pub fn save(path: &Path, profiles: &[Profile]) -> Result<(), AgentError> {
    validate_profiles(profiles)?;
    let temp = path.with_extension("tmp");
    let data =
        serde_json::to_vec_pretty(profiles).map_err(|e| AgentError::config(e.to_string()))?;
    use std::io::Write;
    let mut file = std::fs::File::create(&temp).map_err(|e| AgentError::config(e.to_string()))?;
    file.write_all(&data)
        .and_then(|_| file.sync_all())
        .map_err(|e| AgentError::config(e.to_string()))?;
    drop(file);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(AgentError::config(
                std::io::Error::last_os_error().to_string(),
            ));
        }
    }
    #[cfg(not(windows))]
    std::fs::rename(temp, path).map_err(|e| AgentError::config(e.to_string()))?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_value_and_host_rules() {
        let v=parse("Host *\n Port 2222\nHost example !other\n HostName host.test\n Port 22\n IdentityFile key\n","example").unwrap();
        assert_eq!(v["port"], "2222");
        assert_eq!(v["hostname"], "host.test");
    }
    #[test]
    fn unsupported_fail_closed() {
        assert!(parse("Host example\nProxyCommand unsafe", "example").is_err());
        assert!(parse("Include other", "example").is_err());
    }
    #[test]
    fn profile_atomic_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("profiles.json");
        let v: Vec<Profile> = serde_json::from_str(
            r#"[{"id":"x","name":"Example","host":"host","remotePort":1234,"targetPort":22}]"#,
        )
        .unwrap();
        save(&p, &v).unwrap();
        assert_eq!(load(&p).unwrap(), v);
        let old = std::fs::read(&p).unwrap();
        let mut bad = v;
        bad[0].target_port = 0;
        assert!(save(&p, &bad).is_err());
        assert_eq!(std::fs::read(p).unwrap(), old);
    }
    #[test]
    fn native_folders_ignore_inherited_environment() {
        let expected = format!(
            "FOLDER_PROBE:{}",
            serde_json::json!([home(), data_dir().unwrap()])
        );
        let dir = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "config::tests::native_folder_probe",
                "--nocapture",
            ])
            .env("LOCALAPPDATA", dir.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "Child probe failed: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            String::from_utf8_lossy(&result.stdout).contains(&expected),
            "Folder lookup followed an inherited environment override"
        );
    }
    #[test]
    fn native_folder_probe() {
        assert!(home().is_absolute());
        assert!(data_dir().unwrap().is_absolute());
        println!(
            "FOLDER_PROBE:{}",
            serde_json::json!([home(), data_dir().unwrap()])
        );
    }
}
