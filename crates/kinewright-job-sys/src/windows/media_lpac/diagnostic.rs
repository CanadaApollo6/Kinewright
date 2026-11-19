//! Hosted spike only. Relaxed variants never enter the default launcher.

use super::{
    Child, Inputs, Limits, Mode, begin, check_step, inspect_job, inspect_mitigations, inspect_token,
};
use std::{
    ffi::{OsStr, OsString},
    io,
    path::Path,
    ptr,
};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;

/// Creation settings used by the temporary hosted experiment.
#[derive(Clone, Copy, Debug)]
pub enum Variant {
    Lpac,
    Plain,
    SecurityOnly,
    ParentEnvironment,
}

impl Variant {
    fn mode(self) -> Mode {
        match self {
            Self::Lpac => Mode::Lpac,
            Self::Plain => Mode::Plain,
            Self::SecurityOnly => Mode::SecurityOnly,
            Self::ParentEnvironment => Mode::ParentEnvironment,
        }
    }
}

/// The observations from a successful suspended creation.
pub struct Inspection {
    pub verified: bool,
    pub detail: String,
}

/// Create, inspect, and reap a suspended test fixture without running its code.
///
/// # Errors
/// Returns preflight or creation errors with their API and code.
pub fn inspect(
    program: &Path,
    args: &[OsString],
    inputs: Inputs<'_>,
    limits: Limits,
    variant: Variant,
) -> io::Result<Inspection> {
    let mode = variant.mode();
    let has_job = !matches!(variant, Variant::SecurityOnly);
    let child = begin(
        program,
        args,
        has_job.then_some(inputs),
        limits,
        mode,
        |_, _, _| {},
    )?;
    let process = child
        .process
        .as_ref()
        .ok_or_else(|| io::Error::other("missing suspended process"))?;
    let token = inspect_token(process, &child.sid, mode.opt_out());
    let job = child
        .job
        .as_ref()
        .map(|(job, _)| inspect_job(process, job, &limits));
    let mitigations = inspect_mitigations(process);
    let verified = token.is_ok() && matches!(job, Some(Ok(()))) && mitigations.is_ok();
    let token = match token {
        Ok(()) => format!(
            "AppContainer=true LPAC={} SID=match capabilities=0 integrity=low",
            mode.opt_out()
        ),
        Err(error) => error.to_string(),
    };
    let job = match job {
        Some(Ok(())) => format!(
            "member=true kill-on-close=true active=1 process/job bytes={} CPU hard cap={}",
            limits.private_bytes, limits.cpu_rate
        ),
        Some(Err(error)) => error.to_string(),
        None => {
            let mut member = 0;
            // SAFETY: process is live, null asks about any inherited Job, and member is writable.
            let result = check_step("IsProcessInJob (any Job)", unsafe {
                IsProcessInJob(process.0, ptr::null_mut(), &raw mut member)
            });
            match result {
                Ok(()) => format!("diagnostic Job omitted any Job={}", member != 0),
                Err(error) => format!("diagnostic Job omitted; {error}"),
            }
        }
    };
    Ok(Inspection {
        verified,
        detail: format!(
            "CreateProcessW Win32 0; token=[{token}] Job=[{job}] GetProcessMitigationPolicy={mitigations:?}"
        ),
    })
}

/// Launch a full witness child with the selected diagnostic settings.
///
/// # Errors
/// Refuses missing Jobs, creation errors, and failed inspection.
pub fn launch(
    program: &Path,
    args: &[OsString],
    inputs: Inputs<'_>,
    limits: Limits,
    variant: Variant,
    before_create: impl FnOnce(&OsStr, &OsStr, &OsStr),
) -> io::Result<Child> {
    if matches!(variant, Variant::SecurityOnly) {
        return Err(io::Error::other(
            "security-only variant has no witness Job or pipes",
        ));
    }
    begin(
        program,
        args,
        Some(inputs),
        limits,
        variant.mode(),
        before_create,
    )?
    .verify_and_resume()
}

/// Run the trusted profile-storage canary with the full LPAC creation attributes.
/// The matrix separately reports mitigation inspection so a query failure cannot hide storage.
///
/// # Errors
/// Refuses creation errors or a child without the expected LPAC token and Job.
pub fn launch_profile_canary(
    program: &Path,
    args: &[OsString],
    inputs: Inputs<'_>,
    limits: Limits,
) -> io::Result<Child> {
    let child = begin(
        program,
        args,
        Some(inputs),
        limits,
        Mode::Lpac,
        |_, _, _| {},
    )?;
    let process = child
        .process
        .as_ref()
        .ok_or_else(|| io::Error::other("missing suspended process"))?;
    let (job, _) = child
        .job
        .as_ref()
        .ok_or_else(|| io::Error::other("missing profile canary Job"))?;
    inspect_token(process, &child.sid, true)?;
    inspect_job(process, job, &limits)?;
    child.resume()
}
