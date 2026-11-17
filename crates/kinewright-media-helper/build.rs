fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-z,now");
        if let Ok(root) = std::env::var("FFMPEG_DIR") {
            println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags,-rpath,{root}/lib");
        }
        println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    }
}
