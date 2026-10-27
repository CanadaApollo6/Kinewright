//! Shared native guardian. No agent, `FFmpeg`, GPU, or GUI initialization.
#[path = "../job_guardian/mod.rs"]
mod job_guardian;
fn main() {
    if let Err(error) = job_guardian::main_entry() {
        eprintln!("kinewright-job-guardian: {error}");
        std::process::exit(1);
    }
}
