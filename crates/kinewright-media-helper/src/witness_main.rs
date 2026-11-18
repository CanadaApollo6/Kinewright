#[cfg(target_os = "linux")]
mod avio;
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod bootstrap;
#[cfg(target_os = "linux")]
mod metadata;
#[cfg(target_os = "linux")]
mod probe;
#[cfg(target_os = "linux")]
mod still;

#[cfg(all(target_os = "linux", feature = "slow-tests"))]
mod witness;

#[cfg(all(target_os = "linux", feature = "slow-tests"))]
const WITNESS_EXECUTABLE: bool = true;

fn main() {
    #[cfg(target_os = "linux")]
    if bootstrap::run().is_err() {
        std::process::exit(125);
    }
    #[cfg(not(target_os = "linux"))]
    std::process::exit(125);
}
