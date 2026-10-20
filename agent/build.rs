use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon = root.parent().unwrap().join("assets/Connecty.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numeric = version.replace('.', ",") + ",0";
    let resource = format!(
        r#"101 ICON "{}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 1
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904b0"
  BEGIN
   VALUE "CompanyName", "Chromatic\0"
   VALUE "FileDescription", "Connecty background agent\0"
   VALUE "ProductName", "Connecty\0"
   VALUE "FileVersion", "{version}\0"
   VALUE "ProductVersion", "{version}\0"
   VALUE "LegalCopyright", "Copyright (c) 2026 Chromatic\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x409, 1200
 END
END
"#,
        icon.display().to_string().replace('\\', "/")
    );
    let input = out.join("agent.rc");
    let output = out.join("agent.res");
    fs::write(&input, resource).unwrap();
    let sdk = PathBuf::from(env::var_os("ProgramFiles(x86)").unwrap()).join("Windows Kits/10/bin");
    let mut compilers: Vec<_> = fs::read_dir(sdk)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path().join("x64/rc.exe"))
        .filter(|p| p.is_file())
        .collect();
    compilers.sort();
    let status = Command::new(
        compilers
            .last()
            .expect("Windows SDK resource compiler required"),
    )
    .arg("/nologo")
    .arg("/fo")
    .arg(&output)
    .arg(input)
    .status()
    .unwrap();
    assert!(status.success(), "Agent Windows resources failed");
    println!("cargo:rustc-link-arg-bins={}", output.display());
}
