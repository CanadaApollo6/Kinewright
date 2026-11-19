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
pub enum Variant<'a> {
    Lpac,
    Plain,
    SecurityOnly,
    ParentEnvironment,
    AddedEnvironment(u8),
    ProfileEnvironment { local: &'a Path, lpac: bool },
}

impl<'a> Variant<'a> {
    fn mode(self) -> Mode<'a> {
        match self {
            Self::Lpac => Mode::AppContainer {
                lpac: true,
                local: None,
            },
            Self::Plain => Mode::AppContainer {
                lpac: false,
                local: None,
            },
            Self::SecurityOnly => Mode::SecurityOnly,
            Self::ParentEnvironment => Mode::ParentEnvironment,
            Self::AddedEnvironment(mask) => Mode::AddedEnvironment(mask),
            Self::ProfileEnvironment { local, lpac } => Mode::AppContainer {
                local: Some(local),
                lpac,
            },
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
    variant: Variant<'_>,
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
    let verified = token.verified && matches!(job, Some(Ok(()))) && mitigations.is_ok();
    let token = token.report;
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
    variant: Variant<'_>,
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

/// Run a trusted fixture canary with the AppContainer creation attributes.
/// The matrix separately reports LPAC and mitigation inspection so a query
/// failure cannot hide behavioral results.
///
/// # Errors
/// Refuses creation errors or a child without the expected base AppContainer token and Job.
pub fn launch_profile_canary(
    program: &Path,
    args: &[OsString],
    inputs: Inputs<'_>,
    limits: Limits,
    variant: Variant<'_>,
) -> io::Result<Child> {
    let child = begin(
        program,
        args,
        Some(inputs),
        limits,
        variant.mode(),
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
    let token = inspect_token(process, &child.sid, variant.mode().opt_out());
    eprintln!("suspended canary token: {}", token.report);
    if !token.base_verified {
        return Err(io::Error::other(token.report));
    }
    inspect_job(process, job, &limits)?;
    child.resume()
}

/// Candidate names to supplement `SystemRoot` in the native experiment.
pub const CANDIDATE_ENVIRONMENT: [&str; 7] = [
    "LOCALAPPDATA",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "APPDATA",
    "SystemDrive",
    "windir",
];

pub(super) fn added_environment(mask: u8) -> io::Result<Vec<u16>> {
    let mut entries = ["SystemRoot"]
        .into_iter()
        .chain(
            CANDIDATE_ENVIRONMENT
                .into_iter()
                .enumerate()
                .filter_map(|(bit, key)| (mask & (1 << bit) != 0).then_some(key)),
        )
        .collect::<Vec<_>>();
    entries.sort_by_key(|key| key.to_ascii_uppercase());
    let mut block = Vec::new();
    for key in entries {
        let value = std::env::var_os(key).ok_or_else(|| {
            io::Error::other(format!("CreateProcessW environment: parent lacks {key}"))
        })?;
        let mut entry = OsString::from(key);
        entry.push("=");
        entry.push(value);
        block.extend(super::wide(&entry)?);
    }
    block.push(0);
    Ok(block)
}

/// Holds the key and its suspended process while the host inspects or changes the ACL.
pub struct ProfileRegistryLocation {
    pub name: String,
    _key: RegistryKey,
    _child: Option<super::Suspended>,
}

struct RegistryKey(windows_sys::Win32::System::Registry::HKEY);
impl Drop for RegistryKey {
    fn drop(&mut self) {
        // SAFETY: this guard owns the registry key.
        unsafe { windows_sys::Win32::System::Registry::RegCloseKey(self.0) };
    }
}

/// Open the profile's key in the suspended child's security context, then resolve
/// its name in the host's context. The child never resumes.
///
/// # Errors
/// Returns the failed creation, impersonation, or registry API and code.
pub fn profile_registry_location(
    program: &Path,
    inputs: Inputs<'_>,
    limits: Limits,
    variant: Variant<'_>,
) -> io::Result<ProfileRegistryLocation> {
    use super::{Handle, token};
    use windows_sys::Win32::{
        Security::{
            DuplicateTokenEx, ImpersonateLoggedOnUser, Isolation::GetAppContainerRegistryLocation,
            RevertToSelf, SecurityImpersonation, TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY,
            TokenImpersonation,
        },
        System::Registry::KEY_READ,
    };

    struct Impersonation;
    impl Drop for Impersonation {
        fn drop(&mut self) {
            // SAFETY: only the current thread's impersonation ends.
            if unsafe { RevertToSelf() } == 0 {
                eprintln!("{}", super::last_error("RevertToSelf (profile registry)"));
                std::process::abort();
            }
        }
    }

    let child = begin(
        program,
        &[],
        Some(inputs),
        limits,
        variant.mode(),
        |_, _, _| {},
    )?;
    let token = token::open(
        child.process.as_ref().expect("suspended process"),
        TOKEN_DUPLICATE,
    )?;
    let mut duplicate = ptr::null_mut();
    // SAFETY: the suspended process token is live and duplicate is writable.
    check_step("DuplicateTokenEx (profile registry)", unsafe {
        DuplicateTokenEx(
            token.0,
            TOKEN_QUERY | TOKEN_IMPERSONATE,
            ptr::null(),
            SecurityImpersonation,
            TokenImpersonation,
            &raw mut duplicate,
        )
    })?;
    let duplicate = Handle(duplicate);
    let mut key = ptr::null_mut();
    let status = {
        // SAFETY: the live token came from this test's suspended child.
        check_step("ImpersonateLoggedOnUser (profile registry)", unsafe {
            ImpersonateLoggedOnUser(duplicate.0)
        })?;
        let _revert = Impersonation;
        // SAFETY: key is writable and the thread is impersonating the profile token.
        unsafe { GetAppContainerRegistryLocation(KEY_READ, &raw mut key) }
    };
    if status < 0 {
        let hresult = status.cast_unsigned();
        let code = if hresult >> 16 == 0x8007 {
            format!(", Win32 {}", hresult & 0xffff)
        } else {
            String::new()
        };
        return Err(io::Error::other(format!(
            "GetAppContainerRegistryLocation (host impersonation) HRESULT {hresult:#010x}{code}"
        )));
    }
    if key.is_null() {
        return Err(io::Error::other(
            "GetAppContainerRegistryLocation (host impersonation) HRESULT 0 returned a null key",
        ));
    }
    let key = RegistryKey(key);
    let name = registry_name(&key)?;
    Ok(ProfileRegistryLocation {
        name,
        _key: key,
        _child: Some(child),
    })
}

/// Check the conventional per-user storage path with an ordinary host registry open.
///
/// # Errors
/// Reports a failed registry open or name query with its API and code.
pub fn profile_registry_by_sid(sid: &OsStr) -> io::Result<ProfileRegistryLocation> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, KEY_READ, RegOpenKeyExW};
    let mut path = OsString::from(
        r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppContainer\Storage",
    );
    path.push(r"\");
    path.push(sid);
    let name = super::wide(&path)?;
    let mut key = ptr::null_mut();
    // SAFETY: open a literal key under HKCU; name is terminated and key is writable.
    let code =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, name.as_ptr(), 0, KEY_READ, &raw mut key) };
    if code != 0 {
        return Err(io::Error::other(format!(
            r"RegOpenKeyExW (profile SID lookup, HKCU\{}) Win32 {code}",
            path.to_string_lossy()
        )));
    }
    let key = RegistryKey(key);
    let name = registry_name(&key)?;
    Ok(ProfileRegistryLocation {
        name,
        _key: key,
        _child: None,
    })
}

fn registry_name(key: &RegistryKey) -> io::Result<String> {
    use windows_sys::{
        Wdk::System::Registry::{KeyNameInformation, NtQueryKey},
        Win32::Foundation::RtlNtStatusToDosError,
    };
    let mut output = [0u32; 512];
    let mut written = 0;
    // SAFETY: key is live and the aligned output buffer and length are writable.
    let status = unsafe {
        NtQueryKey(
            key.0,
            KeyNameInformation,
            output.as_mut_ptr().cast(),
            size_of_val(&output)
                .try_into()
                .expect("key name buffer size"),
            &raw mut written,
        )
    };
    if status < 0 {
        // SAFETY: the conversion only takes a numeric NTSTATUS.
        let code = unsafe { RtlNtStatusToDosError(status) };
        return Err(io::Error::other(format!(
            "NtQueryKey (profile registry name) Win32 {code}, NTSTATUS {:#010x}",
            status.cast_unsigned()
        )));
    }
    let bytes = output[0] as usize;
    if bytes == 0
        || !bytes.is_multiple_of(2)
        || bytes + 4 > (written as usize).min(size_of_val(&output))
    {
        return Err(io::Error::other(
            "NtQueryKey (profile registry name) Win32 0 returned an invalid name length",
        ));
    }
    // SAFETY: the bounded UTF-16 name begins after the DWORD length and is aligned.
    let name =
        unsafe { std::slice::from_raw_parts(output.as_ptr().add(1).cast::<u16>(), bytes / 2) };
    Ok(String::from_utf16_lossy(name))
}
