use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/openreel.ico");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR"));
    let icon_path = out_dir.join("OpenReel.ico");
    let resource_script = out_dir.join("OpenReel.rc");
    let compiled_resource = out_dir.join("OpenReel.res");
    let version = env::var("CARGO_PKG_VERSION").expect("Cargo must set CARGO_PKG_VERSION");

    // The committed icon asset is the single source of truth; packaging finds
    // the OUT_DIR copy, so the copy also keeps the installer contract intact.
    fs::copy("assets/openreel.ico", &icon_path)
        .expect("failed to copy assets/openreel.ico to OUT_DIR");
    write_resource_script(&resource_script, &icon_path, &version)
        .expect("failed to generate the OpenReel Windows resource script");

    let output = Command::new("rc.exe")
        .arg("/nologo")
        .arg("/fo")
        .arg(&compiled_resource)
        .arg(&resource_script)
        .output()
        .expect(
            "rc.exe was not found; build from a Visual Studio developer shell or run scripts/setup-ffmpeg.ps1 first",
        );

    assert!(
        output.status.success(),
        "rc.exe failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    println!(
        "cargo:rustc-link-arg-bin=openreel-app={}",
        compiled_resource.display()
    );
}

fn write_resource_script(path: &Path, icon_path: &Path, version: &str) -> std::io::Result<()> {
    let numbers = numeric_version(version);
    let icon_path = icon_path.to_string_lossy().replace('\\', "/");
    let script = format!(
        r#"1 ICON "{icon_path}"

1 VERSIONINFO
 FILEVERSION {major},{minor},{patch},{build}
 PRODUCTVERSION {major},{minor},{patch},{build}
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS 0x40004L
 FILETYPE 0x1L
 FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "OpenReel contributors\0"
            VALUE "FileDescription", "OpenReel video editor\0"
            VALUE "FileVersion", "{version}\0"
            VALUE "InternalName", "OpenReel\0"
            VALUE "LegalCopyright", "Copyright OpenReel contributors\0"
            VALUE "LegalTrademarks", "Licensed under GPL-3.0-only\0"
            VALUE "OriginalFilename", "OpenReel.exe\0"
            VALUE "ProductName", "OpenReel\0"
            VALUE "ProductVersion", "{version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0409, 1200
    END
END
"#,
        major = numbers[0],
        minor = numbers[1],
        patch = numbers[2],
        build = numbers[3],
    );

    fs::write(path, script)
}

fn numeric_version(version: &str) -> [u16; 4] {
    let numeric = version.split_once('-').map_or(version, |(value, _)| value);
    let mut result = [0_u16; 4];

    for (slot, component) in result.iter_mut().zip(numeric.split('.')) {
        *slot = component
            .parse::<u16>()
            .unwrap_or_else(|_| panic!("version component is not a 16-bit integer: {component}"));
    }

    result
}
