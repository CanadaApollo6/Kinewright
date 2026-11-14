use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=KINEWRIGHT_NEMO_DIR");
    let root =
        PathBuf::from(env::var_os("KINEWRIGHT_NEMO_DIR").expect(
            "source scripts/setup-ffmpeg.sh (Windows: scripts/setup-ffmpeg.ps1) before cargo",
        ));
    let marker = root.join(".build-key");
    println!("cargo:rerun-if-changed={}", marker.display());
    let key = fs::read_to_string(marker).expect("NeMo provisioning must finish before cargo");
    let key = key.trim();
    assert!(
        key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid NeMo build key"
    );
    println!("cargo:rustc-env=KINEWRIGHT_NEMO_BUILD_KEY={key}");
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=nemo_speech_asr_c");
}
