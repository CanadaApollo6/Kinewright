//! Shared GPU acquisition for unit tests that must actually run on a device.
//!
//! Every GPU unit test used to carry its own `eprintln!("skipped…"); return;`.
//! A test that skips itself and still reports `ok` is indistinguishable from a
//! test that passed, so a broken shader on a machine with no adapter looked
//! exactly like a green run. These helpers make the default outcome loud: no
//! adapter is a failure with the same remediation text the CC1 fixtures print,
//! and skipping is possible only when the operator asks for it explicitly.

use crate::compositor::GpuContext;

/// Opt-in for a hardware adapter when no software (lavapipe/WARP) adapter
/// exists on this machine. Provenance stays honest: CC1 evidence records the
/// real adapter, `software_fallback=false`, `gpu_claim=true`, and
/// `lane=hardware_optin`.
pub(crate) const HARDWARE_GPU_OPT_IN_ENV: &str = "KINEWRIGHT_CC1_ALLOW_HARDWARE_GPU";

/// Opt-in for silently skipping GPU unit tests. Intended for environments that
/// genuinely cannot host any adapter and knowingly accept reduced coverage.
pub(crate) const GPU_TESTS_MAY_SKIP_ENV: &str = "KINEWRIGHT_GPU_TESTS_MAY_SKIP";

fn may_skip() -> bool {
    std::env::var(GPU_TESTS_MAY_SKIP_ENV).ok().as_deref() == Some("1")
}

fn hardware_opt_in_requested() -> bool {
    std::env::var(HARDWARE_GPU_OPT_IN_ENV).ok().as_deref() == Some("1")
}

/// Acquire an adapter for one GPU unit test.
///
/// The software (lavapipe/WARP) adapter is preferred because it is
/// deterministic; the physical adapter is used only when the operator opts in.
/// Returns `None` only when skipping was explicitly permitted.
///
/// # Panics
///
/// Panics when no usable adapter is available and skipping was not permitted.
pub(crate) fn fixture_gpu_or_skip() -> Option<GpuContext> {
    let software_error = match GpuContext::headless(true) {
        Ok(context) => return Some(context),
        Err(error) => error.to_string(),
    };
    if hardware_opt_in_requested() {
        match GpuContext::headless(false) {
            Ok(context) => return Some(context),
            Err(error) => {
                if may_skip() {
                    eprintln!(
                        "SKIPPED: {GPU_TESTS_MAY_SKIP_ENV}=1 and no adapter at all (software: {software_error}; hardware: {error})"
                    );
                    return None;
                }
                panic!(
                    "{HARDWARE_GPU_OPT_IN_ENV}=1 was set but no adapter was available at all (software: {software_error}; hardware: {error})."
                );
            }
        }
    }
    if may_skip() {
        eprintln!(
            "SKIPPED: {GPU_TESTS_MAY_SKIP_ENV}=1 and no software adapter was available ({software_error})"
        );
        return None;
    }
    panic!(
        "this GPU test requires a lavapipe/WARP fallback adapter; no adapter was available ({software_error}). Install Mesa lavapipe and ensure Vulkan ICD discovery is enabled (for example, VK_ICD_FILENAMES), then rerun cargo test -p kinewright-media. On a machine that has a physical GPU but no software rasterizer, set {HARDWARE_GPU_OPT_IN_ENV}=1 to run on the real adapter. If this environment genuinely cannot host any adapter, set {GPU_TESTS_MAY_SKIP_ENV}=1 to accept the reduced coverage explicitly."
    );
}
