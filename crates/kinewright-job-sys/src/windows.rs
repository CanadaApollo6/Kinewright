use std::{
    ffi::{OsStr, OsString},
    io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
    process::ChildStdin,
    ptr,
};

/// Resolve retained execution state from the account, never its environment.
/// The SID binds both the directory and the lock name to the same account.
///
/// # Errors
/// Every token or known-folder error refuses acquisition.
pub fn execution_namespace() -> io::Result<(PathBuf, String)> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_QUERY, TOKEN_USER,
            TokenUser,
        },
        System::{
            Com::CoTaskMemFree,
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
        UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DONT_UNEXPAND, SHGetKnownFolderPath},
    };
    let mut token = ptr::null_mut();
    // SAFETY: CurrentProcess is a valid pseudo handle and token is writable.
    check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) })?;
    let token = handle(token)?;
    let mut required = 0;
    // SAFETY: The null first buffer requests the exact allocation size.
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &raw mut required) };
    if required < u32::try_from(size_of::<TOKEN_USER>()).map_err(io::Error::other)?
        || required > 65536
    {
        return Err(io::Error::other("account SID unavailable"));
    }
    let mut storage = vec![0usize; (required as usize).div_ceil(size_of::<usize>())];
    // SAFETY: The aligned allocation has at least required writable bytes and
    // remains alive until the SID is converted below.
    check(unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast(),
            required,
            &raw mut required,
        )
    })?;
    // SAFETY: Successful TokenUser populated this correctly aligned structure.
    let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = ptr::null_mut();
    // SAFETY: user.Sid points into the still-live OS-populated token buffer.
    check(unsafe { ConvertSidToStringSidW(user.User.Sid, &raw mut sid) })?;
    // SAFETY: Success returns an allocated NUL-terminated UTF-16 SID string.
    let sid_text = unsafe { copy_os_string(sid) };
    // SAFETY: ConvertSidToStringSidW transfers this LocalAlloc allocation.
    unsafe { LocalFree(sid.cast()) };
    let sid = sid_text
        .into_string()
        .map_err(|_| io::Error::other("invalid account SID"))?;
    if !sid.starts_with("S-1-")
        || !sid
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'S' || b == b'-')
    {
        return Err(io::Error::other("invalid account SID"));
    }
    let mut folder = ptr::null_mut();
    let folder_id = FOLDERID_LocalAppData;
    // SAFETY: The null token selects this account; folder receives an owned shell allocation.
    let result = unsafe {
        SHGetKnownFolderPath(
            &raw const folder_id,
            KF_FLAG_DONT_UNEXPAND.cast_unsigned(),
            ptr::null_mut(),
            &raw mut folder,
        )
    };
    let root = if result >= 0 && !folder.is_null() {
        // SAFETY: Successful SHGetKnownFolderPath returns a NUL-terminated path.
        Some(PathBuf::from(unsafe { copy_os_string(folder) }))
    } else {
        None
    };
    // SAFETY: The API requires CoTaskMemFree even on failure; null is accepted.
    unsafe { CoTaskMemFree(folder.cast()) };
    let root = root.filter(|p| p.is_absolute()).ok_or_else(|| {
        io::Error::other(format!(
            "OS-local machine namespace unavailable: HRESULT {result:#x}"
        ))
    })?;
    Ok((
        root.join("Kinewright")
            .join(format!("studio-execution-{sid}")),
        format!("execution-{sid}.lock"),
    ))
}

unsafe fn copy_os_string(value: *const u16) -> OsString {
    let mut length = 0;
    // SAFETY: Only OS APIs returning owned, NUL-terminated UTF-16 call this helper.
    unsafe {
        while *value.add(length) != 0 {
            length += 1;
        }
        OsString::from_wide(std::slice::from_raw_parts(value, length))
    }
}
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, FILETIME, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0,
    },
    System::{
        JobObjects::{
            CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectBasicProcessIdList,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject,
        },
        Threading::{
            CREATE_SUSPENDED, CreateProcessW, DeleteProcThreadAttributeList,
            EXTENDED_STARTUPINFO_PRESENT, GetProcessTimes, InitializeProcThreadAttributeList,
            LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcess, PROC_THREAD_ATTRIBUTE_JOB_LIST,
            PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE, ResumeThread, STARTUPINFOEXW, TerminateProcess,
            UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: Handle contains one valid owned kernel handle, never duplicated.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn handle(raw: HANDLE) -> io::Result<Handle> {
    if raw.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(Handle(raw))
    }
}
fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut text: Vec<_> = value.encode_wide().collect();
    if text.contains(&0) {
        return Err(io::Error::other("embedded NUL"));
    }
    text.push(0);
    Ok(text)
}

/// # Errors
/// Refuses an inheritable or uninspectable lifetime writer.
pub fn check_lifetime_writer(writer: &ChildStdin) -> io::Result<()> {
    let mut flags = 0;
    // SAFETY: writer keeps the handle alive and flags is writable for this call.
    check(unsafe { GetHandleInformation(writer.as_raw_handle(), &raw mut flags) })?;
    if flags & HANDLE_FLAG_INHERIT != 0 {
        return Err(io::Error::other("inheritable lifetime writer"));
    }
    Ok(())
}

/// A process observer holds no Job handle and cannot extend the Job lifetime.
pub struct ProcessHandle {
    handle: Handle,
    pid: u32,
    start: u64,
}
impl ProcessHandle {
    /// # Errors
    /// Refuses a process without query/synchronize access.
    pub fn open(pid: u32) -> io::Result<Self> {
        // SAFETY: OpenProcess returns a new non-inheritable owned process handle.
        let handle = handle(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        })?;
        let start = creation_time(&handle)?;
        Ok(Self { handle, pid, start })
    }
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }
    #[must_use]
    pub fn start(&self) -> u64 {
        self.start
    }
    /// # Errors
    /// Returns a native wait failure. Windows has no Unix zombie requirement.
    pub fn reaped(&self) -> io::Result<bool> {
        // SAFETY: the observer owns this live process handle. A zero timeout cannot block.
        let status = unsafe { WaitForSingleObject(self.handle.0, 0) };
        if status == u32::MAX {
            Err(io::Error::last_os_error())
        } else {
            Ok(status == WAIT_OBJECT_0)
        }
    }
    /// Kill an exact observed identity without retaining termination authority.
    ///
    /// # Errors
    /// Refuses a replaced identity or denied termination access.
    pub fn kill(&self) -> io::Result<()> {
        // SAFETY: the returned handle is newly owned and never inherited.
        let target = handle(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                0,
                self.pid,
            )
        })?;
        if creation_time(&target)? != self.start {
            return Err(io::Error::other("process identity changed"));
        }
        // SAFETY: target pins the independently verified process identity.
        check(unsafe { TerminateProcess(target.0, 1) })
    }
}
fn creation_time(handle: &Handle) -> io::Result<u64> {
    // SAFETY: FILETIME is a plain integer structure with a valid zero representation.
    let (mut creation, mut exit, mut kernel, mut user): (FILETIME, FILETIME, FILETIME, FILETIME) =
        unsafe { zeroed() };
    // SAFETY: the process handle is owned, and all four output pointers are writable.
    check(unsafe {
        GetProcessTimes(
            handle.0,
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    })?;
    Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

/// The sole lifetime Job owner. This type has no clone or raw-handle accessor.
pub struct Job {
    handle: Handle,
    process: Option<Handle>,
    thread: Option<Handle>,
}
impl Job {
    /// # Errors
    /// Refuses unsupported or unverifiable non-breakaway kill-on-close Jobs.
    pub fn create() -> io::Result<Self> {
        // SAFETY: null security/name creates an unnamed, non-inheritable owned Job.
        let handle = handle(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) })?;
        // SAFETY: this plain Windows information structure has a valid zero representation.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: limits has the exact class and byte size passed to the API.
        check(unsafe {
            SetInformationJobObject(
                handle.0,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .map_err(io::Error::other)?,
            )
        })?;
        let job = Self {
            handle,
            process: None,
            thread: None,
        };
        job.verify()?;
        Ok(job)
    }
    /// # Errors
    /// Refuses inherited handles, missing kill-on-close, or any breakaway flags.
    pub fn verify(&self) -> io::Result<()> {
        let mut flags = 0;
        // SAFETY: the Job is alive and flags is writable.
        check(unsafe { GetHandleInformation(self.handle.0, &raw mut flags) })?;
        // SAFETY: this information structure has a valid zero representation.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        // SAFETY: output buffer matches the information class and size.
        check(unsafe {
            QueryInformationJobObject(
                self.handle.0,
                JobObjectExtendedLimitInformation,
                (&raw mut limits).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .map_err(io::Error::other)?,
                ptr::null_mut(),
            )
        })?;
        if flags & HANDLE_FLAG_INHERIT != 0
            || limits.BasicLimitInformation.LimitFlags != JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        {
            return Err(io::Error::other("unsafe Job limits or inheritance"));
        }
        Ok(())
    }
    /// Create with atomic Job assignment. No user instruction runs before resume.
    ///
    /// # Errors
    /// Refuses unsupported nested Jobs or creation attributes. No spawn fallback.
    pub fn spawn_suspended(&mut self, program: &Path, args: &[String]) -> io::Result<u32> {
        if self.process.is_some() {
            return Err(io::Error::other("Job already started"));
        }
        let application = wide(program.as_os_str())?;
        let text = std::iter::once(program.as_os_str().to_string_lossy().into_owned())
            .chain(args.iter().cloned())
            .map(|arg| quote(&arg))
            .collect::<Vec<_>>()
            .join(" ");
        let mut command = wide(OsStr::new(&text))?;
        let mut bytes = 0;
        // SAFETY: null buffer is the documented size query, with writable size.
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &raw mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
        let pointer: LPPROC_THREAD_ATTRIBUTE_LIST = storage.as_mut_ptr().cast();
        // SAFETY: storage is aligned and has at least the byte size from the size query.
        check(unsafe { InitializeProcThreadAttributeList(pointer, 1, 0, &raw mut bytes) })?;
        let attributes = Attributes {
            pointer,
            _storage: storage,
        };
        let mut jobs = [self.handle.0];
        // SAFETY: jobs and attributes remain alive until CreateProcessW returns.
        check(unsafe {
            UpdateProcThreadAttribute(
                attributes.pointer,
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                jobs.as_mut_ptr().cast(),
                size_of::<HANDLE>(),
                ptr::null_mut(),
                ptr::null(),
            )
        })?;
        // SAFETY: both Windows plain data structures have valid zero representations.
        let (mut startup, mut information): (STARTUPINFOEXW, PROCESS_INFORMATION) =
            unsafe { zeroed() };
        startup.StartupInfo.cb =
            u32::try_from(size_of::<STARTUPINFOEXW>()).map_err(io::Error::other)?;
        startup.lpAttributeList = attributes.pointer;
        // SAFETY: strings are terminated, output is writable, attributes live through this
        // call, and FALSE prevents inheritance of every guardian handle, including the Job.
        check(unsafe {
            CreateProcessW(
                application.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT,
                ptr::null(),
                ptr::null(),
                &raw const startup.StartupInfo,
                &raw mut information,
            )
        })?;
        self.process = Some(handle(information.hProcess)?);
        self.thread = Some(handle(information.hThread)?);
        let mut contained = 0;
        // SAFETY: the process and sole Job handle are both alive, and output is writable.
        check(unsafe {
            IsProcessInJob(
                self.process
                    .as_ref()
                    .ok_or_else(|| io::Error::other("process"))?
                    .0,
                self.handle.0,
                &raw mut contained,
            )
        })?;
        if contained == 0 {
            return Err(io::Error::other("created process lacks Job membership"));
        }
        self.verify()?;
        Ok(information.dwProcessId)
    }
    /// # Errors
    /// Refuses a second resume or a failed native resume.
    pub fn resume(&mut self) -> io::Result<()> {
        let thread = self
            .thread
            .take()
            .ok_or_else(|| io::Error::other("no suspended thread"))?;
        // SAFETY: thread is the retained primary thread of our suspended Job member.
        if unsafe { ResumeThread(thread.0) } != 1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    /// # Errors
    /// Returns a native whole-Job termination failure.
    pub fn kill(&self) -> io::Result<()> {
        // SAFETY: the sole owner retains the Job for the entire operation.
        check(unsafe { TerminateJobObject(self.handle.0, 1) })
    }
    /// # Errors
    /// Missing Job accounting is never emptiness.
    pub fn empty(&self) -> io::Result<bool> {
        // SAFETY: this Windows information structure has a valid zero representation.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
        // SAFETY: output has the exact class and size supplied.
        check(unsafe {
            QueryInformationJobObject(
                self.handle.0,
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                u32::try_from(size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>())
                    .map_err(io::Error::other)?,
                ptr::null_mut(),
            )
        })?;
        Ok(info.ActiveProcesses == 0)
    }
    /// # Errors
    /// A failed wait never proves the direct process has terminated.
    pub fn direct_exited(&self) -> io::Result<bool> {
        let Some(process) = &self.process else {
            return Ok(true);
        };
        // SAFETY: the guardian owns this process handle; the zero timeout cannot block.
        let status = unsafe { WaitForSingleObject(process.0, 0) };
        if status == u32::MAX {
            Err(io::Error::last_os_error())
        } else {
            Ok(status == WAIT_OBJECT_0)
        }
    }
    /// # Errors
    /// Refuses incomplete membership or more than 64 processes.
    pub fn members(&self) -> io::Result<Vec<ProcessHandle>> {
        let mut storage = [0_usize; 66];
        // SAFETY: aligned storage includes the two DWORDs and 64 ULONG_PTR entries.
        check(unsafe {
            QueryInformationJobObject(
                self.handle.0,
                JobObjectBasicProcessIdList,
                storage.as_mut_ptr().cast(),
                u32::try_from(size_of_val(&storage)).map_err(io::Error::other)?,
                ptr::null_mut(),
            )
        })?;
        // SAFETY: the first two fields are DWORDs, regardless of pointer width.
        let counts = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u32>(), 2) };
        if counts[0] != counts[1] || counts[1] > 64 {
            return Err(io::Error::other("incomplete Job membership"));
        }
        let count = counts[1] as usize;
        storage[1..]
            .iter()
            .take(count)
            .map(|pid| ProcessHandle::open(u32::try_from(*pid).map_err(io::Error::other)?))
            .collect()
    }
}
struct Attributes {
    pointer: LPPROC_THREAD_ATTRIBUTE_LIST,
    _storage: Vec<usize>,
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: Initialize succeeded, storage is still alive, and this is the only deletion.
        unsafe {
            DeleteProcThreadAttributeList(self.pointer);
        }
    }
}
fn quote(arg: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in arg.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            '\\',
            if ch == '"' { slashes * 2 + 1 } else { slashes },
        ));
        slashes = 0;
        result.push(ch);
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}

/// # Errors
/// Returns an error for an uninspectable lifetime pipe.
pub fn lifetime_alive() -> io::Result<bool> {
    Ok(peek_input(&std::io::stdin())?.is_some())
}

fn peek_input(stdin: &std::io::Stdin) -> io::Result<Option<u32>> {
    let mut available = 0;
    // SAFETY: stdin owns its input handle. A null buffer with size zero only queries pipe state.
    let result = unsafe {
        windows_sys::Win32::System::Pipes::PeekNamedPipe(
            stdin.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            &raw mut available,
            ptr::null_mut(),
        )
    };
    if result != 0 {
        return Ok(Some(available));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(109) {
        Ok(None)
    } else {
        Err(error)
    }
}

/// The guardian's sole stdin reader never queues a read on an empty synchronous
/// pipe. A concurrent pre-exec peek therefore does not wait behind an idle read.
pub struct ControlInput {
    stdin: std::io::Stdin,
}

#[must_use]
pub fn control_input() -> ControlInput {
    ControlInput {
        stdin: std::io::stdin(),
    }
}

impl io::Read for ControlInput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let Some(available) = peek_input(&self.stdin)? else {
                return Ok(0);
            };
            if available == 0 {
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
            let length = u32::try_from(
                buffer
                    .len()
                    .min(usize::try_from(available).map_err(io::Error::other)?),
            )
            .map_err(io::Error::other)?;
            let mut received = 0;
            // SAFETY: this is the executable's sole reader. The positive peek
            // guarantees at least length bytes, buffer is writable for length,
            // and both output pointers remain valid through the synchronous call.
            let result = unsafe {
                windows_sys::Win32::Storage::FileSystem::ReadFile(
                    self.stdin.as_raw_handle(),
                    buffer.as_mut_ptr(),
                    length,
                    &raw mut received,
                    ptr::null_mut(),
                )
            };
            if result != 0 {
                return usize::try_from(received).map_err(io::Error::other);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(109) {
                return Ok(0);
            }
            return Err(error);
        }
    }
}
