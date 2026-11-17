#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod bootstrap;
#[cfg(target_os = "linux")]
mod bus;
#[cfg(target_os = "linux")]
mod service;

fn main() {
    #[cfg(target_os = "linux")]
    if service::run().is_err() {
        std::process::exit(125);
    }
    #[cfg(not(target_os = "linux"))]
    {
        // The scope supervisor is Linux-only. Slice 1b retains Windows' native path.
        std::process::exit(125);
    }
}
