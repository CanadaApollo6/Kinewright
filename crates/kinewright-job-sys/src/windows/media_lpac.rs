//! Verified `AppContainer` launcher for the Windows media boundary.

use super::{Attributes, Handle, local_app_data, wide};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, PipeReader, PipeWriter},
    mem::{size_of, size_of_val},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::AsRawHandle,
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_INSUFFICIENT_BUFFER,
        ERROR_NOT_ENOUGH_MEMORY, ERROR_TIMEOUT, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
        INVALID_HANDLE_VALUE, SetHandleInformation, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Security::{
        FreeSid, GetTokenInformation, PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES,
        TOKEN_INFORMATION_CLASS,
    },
    Storage::FileSystem::FILE_GENERIC_READ,
    System::{
        IO::{CreateIoCompletionPort, GetQueuedCompletionStatus},
        JobObjects::{
            CreateJobObjectW, IsProcessInJob, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
            JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_ASSOCIATE_COMPLETION_PORT,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
            JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOBOBJECTINFOCLASS, JobObjectAssociateCompletionPortInformation,
            JobObjectBasicAccountingInformation, JobObjectCpuRateControlInformation,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject,
        },
        SystemServices::{
            JOB_OBJECT_MSG_JOB_MEMORY_LIMIT, JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT,
            PROCESS_MITIGATION_DEP_POLICY,
        },
        Threading::{
            CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DETACHED_PROCESS,
            EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess,
            GetProcessMitigationPolicy, INFINITE, InitializeProcThreadAttributeList,
            PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
            PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ProcessASLRPolicy,
            ProcessChildProcessPolicy, ProcessDEPPolicy, ProcessDynamicCodePolicy,
            ProcessExtensionPointDisablePolicy, ProcessImageLoadPolicy,
            ProcessStrictHandleCheckPolicy, ProcessSystemCallDisablePolicy, ResumeThread,
            STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
        },
        WindowsProgramming::{
            PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT,
            PROCESS_CREATION_CHILD_PROCESS_RESTRICTED,
            PROCESS_CREATION_MITIGATION_POLICY_DEP_ENABLE,
        },
    },
};

mod error;
mod profile;
mod security;
mod token;

pub use error::{Code, Error, Step, StorageKind};
pub use profile::{Preparation, Profile, VerifiedProfile};
use security::ChildSecurity;
use token::inspect_token;

const JOB_LIMITS: u32 = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
    | JOB_OBJECT_LIMIT_PROCESS_MEMORY
    | JOB_OBJECT_LIMIT_JOB_MEMORY;
const CPU_FLAGS: u32 = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
const COMPLETION_KEY: usize = 1;

// Creation flags use different bit positions from the policies returned by inspection.
const FORCE_RELOCATE_IMAGES: u64 = 1 << 8;
const STRICT_HANDLE_CHECKS: u64 = 1 << 24;
const WIN32K_SYSTEM_CALL_DISABLE: u64 = 1 << 28;
const EXTENSION_POINT_DISABLE: u64 = 1 << 32;
const PROHIBIT_DYNAMIC_CODE: u64 = 1 << 36;
const IMAGE_LOAD_NO_REMOTE: u64 = 1 << 52;
const IMAGE_LOAD_NO_LOW_LABEL: u64 = 1 << 56;
const MITIGATIONS: u64 = PROCESS_CREATION_MITIGATION_POLICY_DEP_ENABLE as u64
    | FORCE_RELOCATE_IMAGES
    | STRICT_HANDLE_CHECKS
    | WIN32K_SYSTEM_CALL_DISABLE
    | EXTENSION_POINT_DISABLE
    | PROHIBIT_DYNAMIC_CODE
    | IMAGE_LOAD_NO_REMOTE
    | IMAGE_LOAD_NO_LOW_LABEL;

/// The Job charges private commit. CPU units are hundredths of one percent.
#[derive(Clone, Copy)]
pub struct Limits {
    pub private_bytes: usize,
    pub cpu_rate: u32,
}

struct Sid(PSID);
impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: DeriveAppContainerSidFromAppContainerName transfers this SID.
        unsafe { FreeSid(self.0) };
    }
}

/// One verified, resumed child and the sole handle to its unnamed Job.
pub struct Child {
    job: Option<Handle>,
    process: Handle,
    port: Handle,
    pid: u32,
    _local: LaunchDirectory,
}

impl Drop for Child {
    fn drop(&mut self) {
        drop(self.job.take());
        wait_for_exit(&self.process);
    }
}

/// The three handles inherited by the child, in command-line order.
pub struct Inputs<'a> {
    pub source: &'a File,
    pub request: &'a PipeReader,
    pub reply: &'a PipeWriter,
}

/// Launch a verified profile with a fresh host-created environment directory.
///
/// # Errors
/// Refuses unsafe profile state, invalid inputs, native creation errors, or
/// failed suspended inspection. A failed inspection kills the suspended child.
pub fn launch(
    verified: &VerifiedProfile<'_>,
    program: &Path,
    args: &[OsString],
    inputs: Inputs<'_>,
    limits: Limits,
) -> Result<Child, Error> {
    let _lease = profile::PROFILE_CHANGE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (sid, _storage) = verified.0.check()?;
    let local = LaunchDirectory::create(&sid)?;
    let suspended = begin(program, args, inputs, limits, sid, local)
        .map_err(|error| Error::launch(error, Step::Launch, "CreateProcessW"))?;
    suspended
        .verify_and_resume()
        .map_err(|error| Error::launch(error, Step::InspectChild, "suspended child inspection"))
}

struct LaunchDirectory {
    path: PathBuf,
    checked: Option<profile::Node>,
}

impl LaunchDirectory {
    fn create(sid: &Sid) -> Result<Self, Error> {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let parent = local_app_data().map_err(|error| {
            Error::io(
                Step::Environment,
                "SHGetKnownFolderPath",
                &error,
                "LocalAppData",
            )
        })?;
        let root = parent.canonicalize().map_err(|error| {
            Error::io(
                Step::Environment,
                "GetFinalPathNameByHandleW",
                &error,
                parent.display().to_string(),
            )
        })?;
        let root = match root.components().next() {
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_)) => {
                let dos: Vec<_> = root.as_os_str().encode_wide().skip(4).collect();
                PathBuf::from(OsString::from_wide(&dos))
            }
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) => root,
            _ => {
                return Err(Error::unexpected(
                    Step::Environment,
                    "GetFinalPathNameByHandleW",
                    "expected a drive-qualified launch directory",
                ));
            }
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..8 {
            let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!(
                "kinewright-lpac-{}-{now}-{serial}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    let checked = profile::check_launch_directory(&path, sid)?;
                    return Ok(Self {
                        path,
                        checked: Some(checked),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(Error::io(
                        Step::Environment,
                        "CreateDirectoryW",
                        &error,
                        path.display().to_string(),
                    ));
                }
            }
        }
        Err(Error::unexpected(
            Step::Environment,
            "CreateDirectoryW",
            "could not allocate a fresh launch directory",
        ))
    }
}

impl Drop for LaunchDirectory {
    fn drop(&mut self) {
        drop(self.checked.take());
        let _ = fs::remove_dir(&self.path);
    }
}

fn profile_environment(local: &Path) -> io::Result<Vec<u16>> {
    let mut entry = OsString::from("LOCALAPPDATA=");
    entry.push(local);
    let mut block = wide(&entry)?;
    block.extend(system_environment()?);
    Ok(block)
}

fn duplicate_inputs(
    Inputs {
        source,
        request,
        reply,
    }: Inputs<'_>,
) -> io::Result<(Handle, Handle, Handle)> {
    Ok((
        inherit(source.as_raw_handle(), FILE_GENERIC_READ, 0, "source")?,
        inherit(request.as_raw_handle(), 0, DUPLICATE_SAME_ACCESS, "request")?,
        inherit(reply.as_raw_handle(), 0, DUPLICATE_SAME_ACCESS, "reply")?,
    ))
}

struct Suspended {
    process: Option<Handle>,
    thread: Handle,
    job: Option<(Handle, Handle)>,
    pid: u32,
    sid: Sid,
    limits: Limits,
    local: Option<LaunchDirectory>,
    security: ChildSecurity,
}

impl Drop for Suspended {
    fn drop(&mut self) {
        if let Some(process) = &self.process {
            // SAFETY: this process belongs to the launch even if Job inspection failed.
            if unsafe { TerminateProcess(process.0, 1) } == 0 {
                eprintln!(
                    "{}",
                    last_error("TerminateProcess (suspended child cleanup)")
                );
            }
            drop(self.job.take());
            wait_for_exit(process);
        }
    }
}

impl Suspended {
    fn verify_and_resume(self) -> io::Result<Child> {
        let process = self.process.as_ref().expect("suspended process");
        let (job, _) = self.job.as_ref().expect("Job for witnessed launch");
        inspect_token(process, &self.sid)?;
        inspect_job(process, job, &self.limits)?;
        inspect_mitigations(process)?;
        self.security
            .inspect_and_set_defaults(process, &self.thread)?;
        self.resume()
    }

    fn resume(mut self) -> io::Result<Child> {
        // SAFETY: the primary thread handle belongs to the inspected child.
        let previous_count = unsafe { ResumeThread(self.thread.0) };
        if previous_count == u32::MAX {
            return Err(last_error("ResumeThread (verified AppContainer)"));
        }
        expect_value("ResumeThread suspend count", previous_count, 1)?;
        let (job, port) = self.job.take().expect("inspected Job");
        Ok(Child {
            job: Some(job),
            process: self.process.take().expect("inspected process"),
            port,
            pid: self.pid,
            _local: self.local.take().expect("launch directory"),
        })
    }
}

fn begin(
    program: &Path,
    args: &[OsString],
    inputs: Inputs<'_>,
    limits: Limits,
    sid: Sid,
    local: LaunchDirectory,
) -> io::Result<Suspended> {
    let directory = program
        .parent()
        .filter(|path| path.is_absolute() && program.file_name().is_some())
        .ok_or_else(|| io::Error::other("CreateProcessW input: expected an absolute executable"))?;
    if limits.private_bytes == 0 || !(1..=10_000).contains(&limits.cpu_rate) {
        return Err(io::Error::other(
            "SetInformationJobObject input: invalid limits",
        ));
    }
    let application = wide(program.as_os_str())?;
    let cwd = wide(directory.as_os_str())?;
    let environment = profile_environment(&local.path)?;
    let job = configure_job(&limits)?;
    let inherited = duplicate_inputs(inputs)?;
    let handles = [inherited.0.0, inherited.1.0, inherited.2.0];
    let mut command = Vec::new();
    append_arg(&mut command, program.as_os_str())?;
    for handle in handles {
        append_arg(&mut command, OsStr::new(&(handle as usize).to_string()))?;
    }
    for arg in args {
        append_arg(&mut command, arg)?;
    }
    command.push(0);

    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: sid.0,
        ..Default::default()
    };
    let packages = PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;
    let child_policy = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;
    let mitigation = MITIGATIONS;
    let jobs = [job.0.0];
    let attributes = Attributes::media(6)?;
    attributes.add(
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        &capabilities,
        "SECURITY_CAPABILITIES",
    )?;
    attributes.add(
        PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
        &packages,
        "ALL_APPLICATION_PACKAGES_POLICY",
    )?;
    attributes.add(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs, "JOB_LIST")?;
    attributes.add(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &handles, "HANDLE_LIST")?;
    attributes.add(
        PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY,
        &mitigation,
        "MITIGATION_POLICY",
    )?;
    attributes.add(
        PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY,
        &child_policy,
        "CHILD_PROCESS_POLICY",
    )?;
    let security = ChildSecurity::new()?;
    let info = create_suspended(
        &application,
        &mut command,
        &cwd,
        &environment,
        &attributes,
        &security.attributes(),
    )?;
    drop(attributes);
    drop(inherited);
    Ok(Suspended {
        process: Some(Handle(info.hProcess)),
        thread: Handle(info.hThread),
        job: Some(job),
        pid: info.dwProcessId,
        sid,
        limits,
        local: Some(local),
        security,
    })
}

fn system_environment() -> io::Result<Vec<u16>> {
    let root = std::env::var_os("SystemRoot")
        .filter(|root| Path::new(root).is_absolute())
        .ok_or_else(|| {
            io::Error::other("CreateProcessW environment: missing absolute SystemRoot")
        })?;
    let mut entry = OsString::from("SystemRoot=");
    entry.push(root);
    let mut environment = wide(&entry)?;
    environment.push(0);
    Ok(environment)
}

fn create_suspended(
    application: &[u16],
    command: &mut [u16],
    cwd: &[u16],
    environment: &[u16],
    attributes: &Attributes,
    security: &SECURITY_ATTRIBUTES,
) -> io::Result<PROCESS_INFORMATION> {
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>()
        .try_into()
        .map_err(io::Error::other)?;
    startup.lpAttributeList = attributes.pointer;
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: the strings, environment, and attribute values remain live through
    // creation, as does the descriptor behind security. The handle list bounds
    // inheritance and the Job attaches atomically.
    check_step("CreateProcessW (suspended LPAC)", unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            security,
            security,
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | DETACHED_PROCESS,
            environment.as_ptr().cast(),
            cwd.as_ptr(),
            &raw const startup.StartupInfo,
            &raw mut info,
        )
    })?;
    Ok(info)
}

impl Attributes {
    fn media(count: u32) -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: the documented size query writes size and takes a null list.
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &raw mut size) };
        let query = io::Error::last_os_error();
        if query.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER.cast_signed()) || size == 0 {
            return Err(os_error("InitializeProcThreadAttributeList (size)", &query));
        }
        let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
        let pointer = storage.as_mut_ptr().cast();
        // SAFETY: storage is aligned and contains at least size writable bytes.
        check_step(
            "InitializeProcThreadAttributeList (creation entries)",
            unsafe { InitializeProcThreadAttributeList(pointer, count, 0, &raw mut size) },
        )?;
        Ok(Self {
            pointer,
            _storage: storage,
        })
    }

    fn add<T>(&self, attribute: u32, value: &T, name: &str) -> io::Result<()> {
        // SAFETY: launch supplies the matching Windows type for each attribute and
        // keeps each value alive until the list is deleted. Update stores its address.
        let ok = unsafe {
            UpdateProcThreadAttribute(
                self.pointer,
                0,
                attribute as usize,
                ptr::from_ref(value).cast(),
                size_of_val(value),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            return Err(os_error(
                &format!("UpdateProcThreadAttribute ({name})"),
                &error,
            ));
        }
        Ok(())
    }
}

fn append_arg(command: &mut Vec<u16>, arg: &OsStr) -> io::Result<()> {
    const BACKSLASH: u16 = b'\\' as u16;
    const QUOTE: u16 = b'"' as u16;
    if !command.is_empty() {
        command.push(u16::from(b' '));
    }
    command.push(QUOTE);
    let mut slashes = 0;
    for unit in arg.encode_wide() {
        if unit == 0 {
            return Err(io::Error::other("CreateProcessW argument: embedded NUL"));
        }
        if unit == BACKSLASH {
            slashes += 1;
            continue;
        }
        let count = if unit == QUOTE {
            2 * slashes + 1
        } else {
            slashes
        };
        command.extend(std::iter::repeat_n(BACKSLASH, count));
        slashes = 0;
        command.push(unit);
    }
    command.extend(std::iter::repeat_n(BACKSLASH, 2 * slashes));
    command.push(QUOTE);
    Ok(())
}

fn inherit(raw: HANDLE, access: u32, options: u32, step: &str) -> io::Result<Handle> {
    let mut copy = ptr::null_mut();
    // SAFETY: the caller borrows a live file or pipe. copy is a new owned handle.
    let ok = unsafe {
        let owner = GetCurrentProcess();
        DuplicateHandle(owner, raw, owner, &raw mut copy, access, 0, options)
    };
    if ok == 0 {
        let error = io::Error::last_os_error();
        return Err(os_error(&format!("DuplicateHandle ({step})"), &error));
    }
    let handle = Handle(copy);
    // SAFETY: this is the new duplicate, owned only by this launch.
    check_step("SetHandleInformation (listed duplicate)", unsafe {
        SetHandleInformation(handle.0, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT)
    })?;
    let mut flags = 0;
    // SAFETY: the duplicate is live and flags is writable.
    check_step("GetHandleInformation (listed duplicate)", unsafe {
        GetHandleInformation(handle.0, &raw mut flags)
    })?;
    expect_value(
        "GetHandleInformation HANDLE_FLAG_INHERIT",
        flags & HANDLE_FLAG_INHERIT,
        HANDLE_FLAG_INHERIT,
    )?;
    Ok(handle)
}

fn configure_job(limits: &Limits) -> io::Result<(Handle, Handle)> {
    // SAFETY: null security and name create a private, non-inheritable Job.
    let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    let job = own(job, "CreateJobObjectW (LPAC)")?;
    let memory = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        BasicLimitInformation:
            windows_sys::Win32::System::JobObjects::JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_LIMITS,
                ActiveProcessLimit: 1,
                ..Default::default()
            },
        ProcessMemoryLimit: limits.private_bytes,
        JobMemoryLimit: limits.private_bytes,
        ..Default::default()
    };
    set_job(
        &job,
        JobObjectExtendedLimitInformation,
        &memory,
        "extended limits",
    )?;
    let cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
        ControlFlags: CPU_FLAGS,
        Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 {
            CpuRate: limits.cpu_rate,
        },
    };
    set_job(
        &job,
        JobObjectCpuRateControlInformation,
        &cpu,
        "CPU hard cap",
    )?;
    // SAFETY: INVALID_HANDLE_VALUE with a null existing port creates an owned port.
    let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, ptr::null_mut(), 0, 1) };
    let port = own(port, "CreateIoCompletionPort (Job events)")?;
    let association = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
        CompletionKey: COMPLETION_KEY as *mut _,
        CompletionPort: port.0,
    };
    set_job(
        &job,
        JobObjectAssociateCompletionPortInformation,
        &association,
        "completion port",
    )?;
    Ok((job, port))
}

fn set_job<T>(job: &Handle, class: JOBOBJECTINFOCLASS, value: &T, step: &str) -> io::Result<()> {
    // SAFETY: the Job is live and private callers pair each class with its exact buffer type.
    let ok = unsafe {
        SetInformationJobObject(
            job.0,
            class,
            ptr::from_ref(value).cast(),
            size_of_val(value).try_into().expect("Job information size"),
        )
    };
    if ok == 0 {
        let error = io::Error::last_os_error();
        return Err(os_error(
            &format!("SetInformationJobObject ({step})"),
            &error,
        ));
    }
    Ok(())
}

fn query_job<T: Default>(job: &Handle, class: JOBOBJECTINFOCLASS, step: &str) -> io::Result<T> {
    let mut value = T::default();
    // SAFETY: the Job is live and private callers pair the class with its exact output type.
    let ok = unsafe {
        QueryInformationJobObject(
            job.0,
            class,
            ptr::from_mut(&mut value).cast(),
            size_of::<T>().try_into().expect("Job information size"),
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        let error = io::Error::last_os_error();
        return Err(os_error(
            &format!("QueryInformationJobObject ({step})"),
            &error,
        ));
    }
    Ok(value)
}

fn inspect_job(process: &Handle, job: &Handle, limits: &Limits) -> io::Result<()> {
    let mut member = 0;
    // SAFETY: both handles are live and member is writable.
    check_step("IsProcessInJob (suspended LPAC)", unsafe {
        IsProcessInJob(process.0, job.0, &raw mut member)
    })?;
    expect_value("IsProcessInJob membership", member, 1)?;
    let mut flags = 0;
    // SAFETY: the Job is live and flags is writable.
    check_step("GetHandleInformation (LPAC Job)", unsafe {
        GetHandleInformation(job.0, &raw mut flags)
    })?;
    expect_value(
        "GetHandleInformation Job inheritance",
        flags & HANDLE_FLAG_INHERIT,
        0,
    )?;
    let memory: JOBOBJECT_EXTENDED_LIMIT_INFORMATION =
        query_job(job, JobObjectExtendedLimitInformation, "extended limits")?;
    expect_value(
        "QueryInformationJobObject (flags, process count, process memory, Job memory)",
        (
            memory.BasicLimitInformation.LimitFlags,
            memory.BasicLimitInformation.ActiveProcessLimit,
            memory.ProcessMemoryLimit,
            memory.JobMemoryLimit,
        ),
        (JOB_LIMITS, 1, limits.private_bytes, limits.private_bytes),
    )?;
    let cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION =
        query_job(job, JobObjectCpuRateControlInformation, "CPU hard cap")?;
    // SAFETY: the query populates the CPU rate field when hard-cap mode is reported.
    let rate = unsafe { cpu.Anonymous.CpuRate };
    expect_value(
        "QueryInformationJobObject (CPU flags and rate)",
        (cpu.ControlFlags, rate),
        (CPU_FLAGS, limits.cpu_rate),
    )
}

fn read_token(
    token: &Handle,
    class: TOKEN_INFORMATION_CLASS,
    name: &str,
    value: &mut u32,
) -> io::Result<()> {
    let step = format!("GetTokenInformation ({name})");
    let mut size = 0;
    // SAFETY: the fixed-size classes supplied by inspect_token return a DWORD.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            class,
            ptr::from_mut(value).cast(),
            4,
            &raw mut size,
        )
    };
    check_step(&step, ok)?;
    expect_value(&step, size, 4)
}

fn token_buffer(
    token: &Handle,
    class: TOKEN_INFORMATION_CLASS,
    name: &str,
    header: usize,
) -> io::Result<Vec<usize>> {
    let step = format!("GetTokenInformation ({name})");
    let mut size = 0u32;
    // SAFETY: null buffer and writable size use the documented size query.
    unsafe { GetTokenInformation(token.0, class, ptr::null_mut(), 0, &raw mut size) };
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER.cast_signed()) {
        return Err(os_error(&step, &error));
    }
    if !(header..=65_536).contains(&(size as usize)) {
        return Err(io::Error::other(format!(
            "GetTokenInformation {name}: invalid buffer size {size}"
        )));
    }
    let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    // SAFETY: storage is aligned and has at least size writable bytes.
    check_step(&step, unsafe {
        GetTokenInformation(
            token.0,
            class,
            storage.as_mut_ptr().cast(),
            size,
            &raw mut size,
        )
    })?;
    Ok(storage)
}

fn inspect_mitigations(process: &Handle) -> io::Result<()> {
    let mut dep = PROCESS_MITIGATION_DEP_POLICY::default();
    // SAFETY: DEP uses the full policy structure, including its trailing BOOLEAN.
    check_step("GetProcessMitigationPolicy (DEP)", unsafe {
        GetProcessMitigationPolicy(
            process.0,
            ProcessDEPPolicy,
            (&raw mut dep).cast(),
            size_of_val(&dep),
        )
    })?;
    // SAFETY: Windows populated the flags of the DEP policy union.
    expect_value(
        "GetProcessMitigationPolicy DEP Enable",
        unsafe { dep.Anonymous.Flags } & 1,
        1,
    )?;
    let policies = [
        (ProcessASLRPolicy, 1 << 1, "ASLR EnableForceRelocateImages"),
        (
            ProcessStrictHandleCheckPolicy,
            0b11,
            "StrictHandleCheck exceptions and permanent",
        ),
        (
            ProcessSystemCallDisablePolicy,
            1,
            "DisallowWin32kSystemCalls",
        ),
        (
            ProcessExtensionPointDisablePolicy,
            1,
            "DisableExtensionPoints",
        ),
        (ProcessDynamicCodePolicy, 1, "ProhibitDynamicCode"),
        (
            ProcessImageLoadPolicy,
            0b11,
            "NoRemoteImages and NoLowMandatoryLabelImages",
        ),
        (ProcessChildProcessPolicy, 1, "NoChildProcessCreation"),
    ];
    for (policy, mask, name) in policies {
        let mut flags = 0u32;
        let step = format!("GetProcessMitigationPolicy ({name})");
        // SAFETY: each of these policy structures is a single DWORD flags union.
        check_step(&step, unsafe {
            GetProcessMitigationPolicy(
                process.0,
                policy,
                (&raw mut flags).cast(),
                size_of_val(&flags),
            )
        })?;
        expect_value(&step, flags & mask, mask)?;
    }
    Ok(())
}

impl Child {
    /// # Errors
    /// Returns a native wait or exit-status failure, including timeout.
    pub fn wait(&self, timeout: Duration) -> io::Result<u32> {
        wait_process(&self.process, timeout)
    }

    /// Observe a memory event for this child, kill the Job, and verify emptiness.
    /// Windows can deny a commit without stopping the process.
    ///
    /// # Errors
    /// Returns a missing event, native termination failure, or nonempty Job.
    pub fn terminate_on_memory_limit(&self, timeout: Duration) -> io::Result<u32> {
        let start = Instant::now();
        loop {
            let mut message = 0;
            let mut key = 0;
            let mut payload = ptr::null_mut();
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Err(os_error(
                    "GetQueuedCompletionStatus (Job memory limit)",
                    &io::Error::from_raw_os_error(ERROR_TIMEOUT.cast_signed()),
                ));
            }
            // SAFETY: the owned port is live and all outputs are writable.
            check_step("GetQueuedCompletionStatus (Job memory limit)", unsafe {
                GetQueuedCompletionStatus(
                    self.port.0,
                    &raw mut message,
                    &raw mut key,
                    &raw mut payload,
                    milliseconds(remaining),
                )
            })?;
            if key == COMPLETION_KEY
                && payload as usize == self.pid as usize
                && matches!(
                    message,
                    JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT | JOB_OBJECT_MSG_JOB_MEMORY_LIMIT
                )
            {
                break;
            }
        }
        // SAFETY: the verified child is contained by this live Job.
        let job = self
            .job
            .as_ref()
            .ok_or_else(|| io::Error::other("missing child Job"))?;
        check_step("TerminateJobObject (observed memory limit)", unsafe {
            TerminateJobObject(job.0, ERROR_NOT_ENOUGH_MEMORY)
        })?;
        let exit = self.wait(timeout)?;
        let accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = query_job(
            job,
            JobObjectBasicAccountingInformation,
            "after memory termination",
        )?;
        expect_value(
            "QueryInformationJobObject ActiveProcesses after termination",
            accounting.ActiveProcesses,
            0,
        )?;
        Ok(exit)
    }

    /// Close the sole Job handle and observe its effect through the process handle.
    /// Teardown waits for exit before releasing the checked launch directory.
    ///
    /// # Errors
    /// Returns a native wait or exit-status failure, including timeout.
    pub fn close_job(mut self, timeout: Duration) -> io::Result<u32> {
        let job = self
            .job
            .take()
            .ok_or_else(|| io::Error::other("missing child Job"))?;
        // SAFETY: the child owns the sole Job handle and closes it once here.
        check_step("CloseHandle (sole LPAC Job)", unsafe { CloseHandle(job.0) })?;
        std::mem::forget(job);
        wait_process(&self.process, timeout)
    }
}

fn wait_for_exit(process: &Handle) {
    // SAFETY: the live process handle grants SYNCHRONIZE. Closing its Job starts termination.
    if unsafe { WaitForSingleObject(process.0, INFINITE) } != WAIT_OBJECT_0 {
        eprintln!(
            "{}",
            last_error("WaitForSingleObject (LPAC directory cleanup)")
        );
    }
}

fn wait_process(process: &Handle, timeout: Duration) -> io::Result<u32> {
    // SAFETY: this live process handle has SYNCHRONIZE access.
    let status = unsafe { WaitForSingleObject(process.0, milliseconds(timeout)) };
    if status == WAIT_FAILED {
        return Err(last_error("WaitForSingleObject (LPAC child)"));
    }
    if status == WAIT_TIMEOUT {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("WaitForSingleObject (LPAC child): Win32 {WAIT_TIMEOUT}"),
        ));
    }
    expect_value("WaitForSingleObject (LPAC child)", status, WAIT_OBJECT_0)?;
    let mut exit = 0;
    // SAFETY: the process handle is live and exit is writable.
    check_step("GetExitCodeProcess (LPAC child)", unsafe {
        GetExitCodeProcess(process.0, &raw mut exit)
    })?;
    Ok(exit)
}

fn milliseconds(timeout: Duration) -> u32 {
    u32::try_from(timeout.as_millis().min(u128::from(u32::MAX - 1))).expect("bounded timeout")
}

fn expect_value<T: std::fmt::Debug + PartialEq + Copy>(
    step: &str,
    actual: T,
    expected: T,
) -> io::Result<()> {
    if actual == expected {
        return Ok(());
    }
    Err(io::Error::other(Error::unexpected(
        Step::InspectChild,
        step,
        format!("observed {actual:?}, expected {expected:?}"),
    )))
}

fn own(raw: HANDLE, step: &str) -> io::Result<Handle> {
    if raw.is_null() {
        Err(last_error(step))
    } else {
        Ok(Handle(raw))
    }
}

fn check_step(step: &str, success: i32) -> io::Result<()> {
    if success == 0 {
        Err(last_error(step))
    } else {
        Ok(())
    }
}

fn last_error(step: &str) -> io::Error {
    os_error(step, &io::Error::last_os_error())
}

fn os_error(step: &str, error: &io::Error) -> io::Error {
    io::Error::other(Error::io(Step::Launch, step, error, error.to_string()))
}
