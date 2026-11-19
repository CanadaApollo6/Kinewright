#![cfg(windows)]
#![allow(unsafe_code)]

use kinewright_job_sys::media_lpac::{self, Limits, SPIKE_CONTAINER_NAME};
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, PipeReader, PipeWriter, Read, Write},
    net::{Ipv4Addr, TcpListener},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Component, Path, PathBuf, Prefix},
    process::{Command, Stdio},
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_ACCESS_DENIED, ERROR_ACCESS_DISABLED_BY_POLICY, ERROR_CHILD_PROCESS_BLOCKED,
        ERROR_COMMITMENT_LIMIT, ERROR_FILE_NOT_FOUND, ERROR_NOT_ENOUGH_MEMORY,
        ERROR_NOT_ENOUGH_QUOTA, ERROR_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_WORKING_SET_QUOTA,
        INVALID_HANDLE_VALUE, LocalFree, MAX_PATH, WAIT_TIMEOUT,
    },
    Security::{
        ACL,
        Authorization::{
            ConvertSidToStringSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW,
            SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
            TRUSTEE_IS_USER,
        },
        DACL_SECURITY_INFORMATION, FreeSid,
        Isolation::{DeriveAppContainerSidFromAppContainerName, GetAppContainerFolderPath},
        PSECURITY_DESCRIPTOR, PSID, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    },
    Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, PIPE_ACCESS_INBOUND,
    },
    System::{
        Com::CoTaskMemFree,
        Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PeekNamedPipe},
        Threading::{ALL_PROCESSOR_GROUPS, DETACHED_PROCESS, GetActiveProcessorCount},
    },
};

const TIMEOUT: Duration = Duration::from_secs(8);
const MEMORY_BYTES: usize = 64 * 1024 * 1024;
const SOURCE: &[u8] = b"LPAC source bytes";
const EXIT_AFTER_MEMORY_LIMIT: u32 = ERROR_NOT_ENOUGH_MEMORY;
const FILE_DENIALS: &[u32] = &[ERROR_ACCESS_DENIED];
static SERIAL: AtomicU64 = AtomicU64::new(0);
// std::process::Command and the LPAC launcher briefly own inheritable duplicates.
static SPAWNING: Mutex<()> = Mutex::new(());

#[test]
fn suspended_lpac_passes_inspection_and_runs_with_minimal_environment() {
    let fixture = Fixture::new();
    let unconfined = fixture.startup(false);
    let confined = fixture.startup(true);
    assert!(
        unconfined.is_ok() && confined.is_ok(),
        "unconfined startup: {unconfined:?}; LPAC startup: {confined:?}"
    );
}

#[test]
fn inherited_source_is_readable_but_read_only_through_the_handle_list() {
    let fixture = Fixture::new();
    fixture.single("source", "", true).expect(
        &["WriteFile(read-only inherited source)"],
        FILE_DENIALS,
        "read source sentinel then try writing through the inherited handle",
    );
    assert_eq!(fs::read(&fixture.source).unwrap(), SOURCE);
}

#[test]
fn image_directory_refuses_create_growth_and_mutation() {
    let fixture = Fixture::new();
    fixture.refuse_writes(fixture.image_dir());
}

#[test]
fn user_temp_directory_refuses_create_growth_and_mutation() {
    let fixture = Fixture::new();
    fixture.refuse_writes(&std::env::temp_dir());
}

#[test]
fn user_profile_directory_refuses_create_growth_and_mutation() {
    let fixture = Fixture::new();
    let profile = env_path("USERPROFILE");
    fixture.refuse_writes(&profile);
}

#[test]
fn existing_appcontainer_storage_refuses_create_growth_and_mutation() {
    let fixture = Fixture::new();
    let mut child = fixture.start("hold", "", true);
    child.reply().expect(
        &["ReadFile(hold)"],
        &[0],
        "start LPAC before storage discovery",
    );
    let mut candidates = BTreeSet::new();
    let local = env_path("LOCALAPPDATA");
    let packages = local.join("Packages");
    let sid = fixture.sid.text();
    candidates.insert(packages.join(SPIKE_CONTAINER_NAME));
    candidates.insert(packages.join(&sid));
    if let Some(path) = container_storage(&sid) {
        assert_ne!(
            path, local,
            "GetAppContainerFolderPath returned the account's LocalAppData"
        );
        candidates.insert(path);
    }

    let mut checked = BTreeSet::new();
    for path in candidates {
        check_storage_tree(&fixture, &path, &mut checked);
    }
    if checked.is_empty() {
        eprintln!("LPAC storage: no existing AppContainer directory for {sid:?}");
    }
    child.release();
    assert_eq!(child.finish(), 0, "storage discovery child exit");
}

#[test]
fn current_user_registry_key_creation_is_refused() {
    let fixture = Fixture::new();
    fixture.refused(
        "registry",
        unique(),
        &["RegCreateKeyExW(HKCU)"],
        FILE_DENIALS,
    );
}

#[test]
fn loopback_tcp_connection_is_refused() {
    let fixture = Fixture::new();
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback control listener");
    fixture.refused(
        "connect",
        listener.local_addr().unwrap().to_string(),
        &["connect"],
        &[ERROR_ACCESS_DENIED, 10013], // WSAEACCES
    );
    listener.set_nonblocking(true).unwrap();
    listener
        .accept()
        .expect("accept the unconfined control connection");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock,
        "the listener received another connection"
    );
}

#[test]
fn opening_the_host_for_handle_duplication_is_refused() {
    let fixture = Fixture::new();
    fixture.refused(
        "process",
        std::process::id().to_string(),
        &["OpenProcess(PROCESS_DUP_HANDLE)"],
        FILE_DENIALS,
    );
}

#[test]
fn opening_the_host_for_limited_query_is_refused() {
    let fixture = Fixture::new();
    fixture.refused(
        "process-query",
        std::process::id().to_string(),
        &["OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)"],
        FILE_DENIALS,
    );
}

#[test]
fn opening_a_default_descriptor_named_pipe_is_refused() {
    let fixture = Fixture::new();
    let name = format!(r"\\.\pipe\{}", unique());
    let text = wide(&name);
    let _first = default_pipe_instance(&text);
    // The control can leave its instance connected until the host closes it.
    let _second = default_pipe_instance(&text);
    fixture.refused("pipe", name, &["CreateFileW(named pipe)"], FILE_DENIALS);
}

fn default_pipe_instance(name: &[u16]) -> OwnedHandle {
    // SAFETY: the name is terminated, and null security selects the default descriptor.
    let pipe = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_INBOUND,
            PIPE_REJECT_REMOTE_CLIENTS,
            2,
            4096,
            4096,
            0,
            ptr::null(),
        )
    };
    assert_ne!(
        pipe,
        INVALID_HANDLE_VALUE,
        "CreateNamedPipeW (default descriptor): {}",
        io::Error::last_os_error()
    );
    // SAFETY: success transfers ownership of a new pipe instance.
    unsafe { OwnedHandle::from_raw_handle(pipe) }
}

#[test]
fn starting_a_second_process_is_refused() {
    let fixture = Fixture::new();
    fixture.refused(
        "spawn",
        &fixture.exe,
        &["CreateProcessW(second process)"],
        &[
            ERROR_CHILD_PROCESS_BLOCKED,
            ERROR_ACCESS_DENIED,
            ERROR_ACCESS_DISABLED_BY_POLICY,
        ],
    );
}

#[test]
fn a_job_memory_event_ends_a_child_that_exceeds_the_commit_cap() {
    let fixture = Fixture::new();
    let bytes = (MEMORY_BYTES * 2).to_string();
    let mut open = fixture.start("memory", &bytes, false);
    open.reply()
        .expect(&["VirtualAlloc"], &[0], "unconfined child commits 128 MiB");
    open.release();
    assert_eq!(open.finish(), 0, "unconfined memory control exit");
    let mut confined = fixture.start("memory", &bytes, true);
    let outcome = confined.reply();
    outcome.expect(
        &["VirtualAlloc"],
        &[
            ERROR_NOT_ENOUGH_MEMORY,
            ERROR_COMMITMENT_LIMIT,
            ERROR_WORKING_SET_QUOTA,
            ERROR_NOT_ENOUGH_QUOTA,
        ],
        "LPAC child exceeds its 64 MiB Job commit limit",
    );
    let Process::Lpac(child) = &confined.process else {
        unreachable!()
    };
    let code = child
        .terminate_on_memory_limit(TIMEOUT)
        .unwrap_or_else(|error| {
            panic!("Job memory event / termination after {outcome:?}: {error}")
        });
    assert_eq!(
        code, EXIT_AFTER_MEMORY_LIMIT,
        "GetExitCodeProcess after Job memory event"
    );
}

#[test]
fn closing_the_last_job_handle_ends_a_running_child() {
    let fixture = Fixture::new();
    let mut running = fixture.start("hold", "", true);
    running.reply().expect(
        &["ReadFile(hold)"],
        &[0],
        "child waits with request pipe open",
    );
    let Process::Lpac(child) = running.process else {
        unreachable!()
    };
    let result = child.wait(Duration::ZERO).unwrap_err();
    assert!(
        result.kind() == io::ErrorKind::TimedOut,
        "WaitForSingleObject before Job close expected WAIT_TIMEOUT: {result}"
    );
    let exit = child
        .close_job(TIMEOUT)
        .unwrap_or_else(|e| panic!("CloseHandle(Job) then wait: {e}"));
    eprintln!("CloseHandle(Job): child signaled with exit code {exit:#010x}");
}

struct Fixture {
    exe: PathBuf,
    image_bytes: u64,
    source: PathBuf,
    sid: Sid,
    _root: Temp,
}

impl Fixture {
    fn new() -> Self {
        let root = Temp::new();
        let directory = root.0.join("image directory");
        fs::create_dir(&directory).expect("CreateDirectoryW (private fixture image directory)");
        let sid = Sid::derive();
        grant_image_access(&directory, &sid);
        let exe = directory.join("fixture.exe");
        // Create the copy with the directory's inheritable ACL.
        let input = env!("CARGO_BIN_EXE_kinewright-lpac-spike-child");
        let image_bytes = copy_image(Path::new(input), &exe);
        let source = root.0.join("source");
        fs::write(&source, SOURCE).expect("CreateFileW/WriteFile (fixture source)");
        Self {
            exe,
            image_bytes,
            source,
            sid,
            _root: root,
        }
    }

    fn image_dir(&self) -> &Path {
        self.exe.parent().unwrap()
    }

    fn inspect_image(&self, application: &OsStr) {
        let file = File::open(application)
            .unwrap_or_else(|error| panic!("CreateFileW (fixture image preflight): {error}"));
        let image = file.metadata().unwrap_or_else(|error| {
            panic!("GetFileInformationByHandle (fixture image preflight): {error}")
        });
        eprintln!(
            "GetFileInformationByHandle (fixture image preflight) Win32 0: file={} bytes={} copied={}",
            image.is_file(),
            image.len(),
            self.image_bytes
        );
        assert!(
            image.is_file() && image.len() == self.image_bytes,
            "GetFileInformationByHandle (fixture image preflight) Win32 0: expected a {} byte file, got file={} bytes={}",
            self.image_bytes,
            image.is_file(),
            image.len()
        );
    }

    #[expect(
        clippy::unnecessary_debug_formatting,
        reason = "Preserve the exact Windows strings, including quotes and escaped code units"
    )]
    fn inspect_launch(&self, case: &str, application: &OsStr, command_line: &OsStr, cwd: &OsStr) {
        eprintln!(
            "CreateProcessW (suspended LPAC, {case}) application={application:?} command_line={command_line:?} cwd={cwd:?} verbatim(application,argv0,cwd)={:?}",
            (verbatim(application), verbatim(command_line), verbatim(cwd))
        );
        self.inspect_image(application);
        let directory = fs::metadata(cwd).unwrap_or_else(|error| {
            panic!("GetFileAttributesExW (LPAC current directory preflight): {error}")
        });
        assert!(
            directory.is_dir(),
            "GetFileAttributesExW (LPAC current directory preflight) Win32 0: expected a directory"
        );
    }

    fn start(&self, case: &str, target: impl AsRef<OsStr>, lpac: bool) -> Running {
        self.try_start(case, target, lpac)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn try_start(
        &self,
        case: &str,
        target: impl AsRef<OsStr>,
        lpac: bool,
    ) -> Result<Running, String> {
        let (child_request, mut request) =
            io::pipe().map_err(|error| format!("{case} CreatePipe (fixture request): {error}"))?;
        let (reply, child_reply) =
            io::pipe().map_err(|error| format!("{case} CreatePipe (fixture reply): {error}"))?;
        let process = {
            // The lock serializes temporary handle inheritance and guards no data.
            let _spawning = SPAWNING
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if lpac {
                let source = File::open(&self.source)
                    .map_err(|error| format!("{case} CreateFileW (read-only source): {error}"))?;
                // SAFETY: querying all processor groups needs no pointer or handle.
                let cpus = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
                if cpus == 0 {
                    return Err(format!(
                        "{case} GetActiveProcessorCount: {}",
                        io::Error::last_os_error()
                    ));
                }
                let limits = Limits {
                    private_bytes: MEMORY_BYTES,
                    cpu_rate: 10_000u32.div_ceil(cpus),
                };
                let args = [OsString::from(case), target.as_ref().to_os_string()];
                media_lpac::launch(
                    &self.exe,
                    &args,
                    &source,
                    &child_request,
                    &child_reply,
                    limits,
                    |application, command_line, cwd| {
                        self.inspect_launch(case, application, command_line, cwd);
                    },
                )
                .map(Process::Lpac)
            } else {
                Command::new(&self.exe)
                    .args(["0", "0", "0", case])
                    .arg(target.as_ref())
                    .env_clear()
                    .env("SystemRoot", env_path("SystemRoot"))
                    .current_dir(self.image_dir())
                    .creation_flags(DETACHED_PROCESS)
                    .stdin(Stdio::from(child_request))
                    .stdout(Stdio::from(child_reply))
                    .stderr(Stdio::null())
                    .spawn()
                    .map(|child| Process::Open(Control(child)))
            }
        };
        let step = if lpac {
            "LPAC launch/inspection"
        } else {
            "unconfined CreateProcessW"
        };
        let process = process.map_err(|error| format!("{case} {step}: {error}"))?;
        request
            .write_all(b"!")
            .map_err(|error| format!("{case} {step} succeeded, WriteFile(request): {error}"))?;
        Ok(Running {
            process,
            request,
            reply,
            context: format!("{case} lpac={lpac}"),
        })
    }

    fn single(&self, case: &str, target: impl AsRef<OsStr>, lpac: bool) -> Report {
        self.try_single(case, target, lpac)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn try_single(
        &self,
        case: &str,
        target: impl AsRef<OsStr>,
        lpac: bool,
    ) -> Result<Report, String> {
        let mut child = self.try_start(case, target, lpac)?;
        let report = child.try_reply()?;
        let exit = child.try_finish()?;
        if exit != 0 {
            return Err(format!(
                "GetExitCodeProcess ({case}, lpac={lpac}) exit {exit:#010x}, reply {report:?}"
            ));
        }
        Ok(report)
    }

    fn startup(&self, lpac: bool) -> Result<(), String> {
        let mode = if lpac { "LPAC" } else { "unconfined" };
        if !lpac {
            eprintln!(
                "unconfined startup CreateProcessW input image=[{}] cwd=[{}]",
                self.exe.display(),
                self.image_dir().display()
            );
        }
        let result = self
            .try_single("ready", self.image_dir(), lpac)
            .and_then(|report| {
                if report.api == "ready" && report.code == 0 {
                    Ok(())
                } else {
                    Err(format!(
                        "{} returned Win32 {} ({:#010x})",
                        report.api, report.code, report.code
                    ))
                }
            });
        match &result {
            Ok(()) => eprintln!(
                "{mode} startup: CreateProcessW and detached startup passed (Win32 0, exit 0)"
            ),
            Err(error) => eprintln!("{mode} startup: {error}"),
        }
        result
    }

    fn refused(&self, case: &str, target: impl AsRef<OsStr>, apis: &[&str], codes: &[u32]) {
        self.single(case, target.as_ref(), false)
            .expect(apis, &[0], "unconfined control");
        self.single(case, target, true)
            .expect(apis, codes, "LPAC refusal");
    }

    fn refuse_writes(&self, parent: &Path) {
        let create = Canary::new(parent);
        self.single("create", &create.0, false).expect(
            &["WriteFile(canary)"],
            &[0],
            "unconfined create",
        );
        assert_eq!(fs::read(&create.0).unwrap(), b"canary");
        fs::remove_file(&create.0).expect("remove unconfined create canary");
        self.file_refusal("create", &create.0);
        assert!(!create.0.exists(), "LPAC created {}", create.0.display());

        let existing = Canary::new(parent);
        for (case, expected) in [
            ("grow", b"baselinecanary".as_slice()),
            ("mutate", b"canaryne".as_slice()),
        ] {
            fs::write(&existing.0, b"baseline").expect("seed write canary");
            self.single(case, &existing.0, false).expect(
                &["WriteFile(canary)"],
                &[0],
                "unconfined write",
            );
            assert_eq!(fs::read(&existing.0).unwrap(), expected);
            fs::write(&existing.0, b"baseline").expect("reset write canary");
            self.file_refusal(case, &existing.0);
            assert_eq!(fs::read(&existing.0).unwrap(), b"baseline");
        }
    }

    fn file_refusal(&self, case: &str, target: &Path) {
        let report = self.single(case, target, true);
        report.expect(
            &["CreateFileW(canary)", "WriteFile(canary)"],
            FILE_DENIALS,
            &format!("{case} LPAC file refusal at {}", target.display()),
        );
    }
}

fn copy_image(input: &Path, image: &Path) -> u64 {
    let mut input = File::open(input).unwrap_or_else(|e| {
        panic!(
            "CreateFileW (open archived fixture): {e}; input=[{}]",
            input.display()
        )
    });
    let bytes = input
        .metadata()
        .unwrap_or_else(|e| panic!("GetFileInformationByHandle (archived fixture): {e}"))
        .len();
    let mut output = File::create_new(image)
        .unwrap_or_else(|e| panic!("CreateFileW (private fixture image): {e}"));
    let copied = io::copy(&mut input, &mut output)
        .unwrap_or_else(|e| panic!("ReadFile/WriteFile (copy fixture): {e}"));
    assert!(
        bytes > 0 && bytes == copied,
        "ReadFile/WriteFile (copy fixture) Win32 0: source={bytes} copied={copied}"
    );
    copied
}

fn verbatim(value: &OsStr) -> bool {
    let mut units = value.encode_wide().peekable();
    if units.peek() == Some(&u16::from(b'"')) {
        units.next();
    }
    units.take(4).eq(r"\\?\".encode_utf16())
}

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let reported = std::env::temp_dir();
        let resolved = reported.canonicalize().unwrap_or_else(|error| {
            panic!(
                "GetFinalPathNameByHandleW (resolve fixture temp): {error}; temp=[{}]",
                reported.display()
            )
        });
        // These fixtures use short, drive-qualified DOS names for both CreateProcessW inputs.
        let local = match resolved.components().next() {
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_)) => {
                let units: Vec<u16> = resolved.as_os_str().encode_wide().skip(4).collect();
                PathBuf::from(OsString::from_wide(&units))
            }
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) => resolved,
            _ => panic!(
                "GetFinalPathNameByHandleW (fixture temp) Win32 0: expected local drive, got [{}]",
                resolved.display()
            ),
        };
        let path = local.join(unique());
        let units = path
            .join("image directory")
            .join("fixture.exe")
            .as_os_str()
            .encode_wide()
            .count();
        assert!(
            units < MAX_PATH as usize,
            "GetFinalPathNameByHandleW (fixture path preflight) Win32 0: expected short DOS path, got {units} UTF-16 units"
        );
        eprintln!(
            "GetFinalPathNameByHandleW (fixture temp) Win32 0: reported=[{}] local=[{}]",
            reported.display(),
            local.display()
        );
        fs::create_dir(&path)
            .unwrap_or_else(|e| panic!("CreateDirectoryW (private fixture temp): {e}"));
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let start = Instant::now();
        loop {
            match fs::remove_dir_all(&self.0) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                // Job closure may precede release of the child's image section.
                Err(_) if start.elapsed() < Duration::from_secs(2) => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    eprintln!("RemoveDirectoryW (fixture {}): {e}", self.0.display());
                    assert!(thread::panicking(), "could not clean fixture directory");
                    break;
                }
            }
        }
    }
}

struct Canary(PathBuf);
impl Canary {
    fn new(parent: &Path) -> Self {
        Self(parent.join(unique()))
    }
}
impl Drop for Canary {
    fn drop(&mut self) {
        match fs::remove_file(&self.0) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("DeleteFileW (canary {}): {e}", self.0.display()),
        }
    }
}

fn unique() -> String {
    format!(
        "Kinewright-LpacSpike-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
}

fn env_path(key: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(key).unwrap_or_else(|| panic!("{key} is missing")));
    assert!(path.is_absolute(), "{key} is not an absolute directory");
    path
}

enum Process {
    Lpac(media_lpac::Child),
    Open(Control),
}

struct Control(std::process::Child);
impl Drop for Control {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

struct Running {
    process: Process,
    request: PipeWriter,
    reply: PipeReader,
    context: String,
}

impl Running {
    fn reply(&mut self) -> Report {
        self.try_reply().unwrap_or_else(|error| panic!("{error}"))
    }

    fn try_reply(&mut self) -> Result<Report, String> {
        let start = Instant::now();
        let mut bytes = Vec::new();
        while start.elapsed() < TIMEOUT {
            let mut available = 0;
            // SAFETY: reply is the live pipe reader and available is writable.
            let ok = unsafe {
                PeekNamedPipe(
                    self.reply.as_raw_handle(),
                    ptr::null_mut(),
                    0,
                    ptr::null_mut(),
                    &raw mut available,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                let error = io::Error::last_os_error();
                return Err(match &mut self.process {
                    Process::Lpac(child) => format!(
                        "{} PeekNamedPipe(reply): {error}; child wait/exit {:?}",
                        self.context,
                        child
                            .wait(Duration::from_millis(100))
                            .map(|code| format!("{code:#010x}"))
                    ),
                    Process::Open(child) => format!(
                        "{} PeekNamedPipe(reply): {error}; unconfined exit {:?}",
                        self.context,
                        child.0.try_wait()
                    ),
                });
            }
            if available == 0 {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            let length = bytes.len();
            let available = available as usize;
            if length + available > 512 {
                return Err(format!(
                    "{} PeekNamedPipe (reply) Win32 0: unexpected reply length",
                    self.context
                ));
            }
            bytes.resize(length + available, 0);
            self.reply
                .read_exact(&mut bytes[length..])
                .map_err(|error| format!("{} ReadFile (fixture reply): {error}", self.context))?;
            if bytes.ends_with(b"\n") {
                return Report::parse(&self.context, &bytes);
            }
        }
        Err(format!(
            "{} PeekNamedPipe(reply): expected a report within {TIMEOUT:?} (timeout Win32 {WAIT_TIMEOUT})",
            self.context
        ))
    }

    fn release(&mut self) {
        self.request
            .write_all(b"!")
            .expect("WriteFile (release fixture)");
    }

    fn finish(self) -> u32 {
        self.try_finish().unwrap_or_else(|error| panic!("{error}"))
    }

    fn try_finish(mut self) -> Result<u32, String> {
        match &mut self.process {
            Process::Lpac(child) => child
                .wait(TIMEOUT)
                .map_err(|error| format!("{}: {error}", self.context)),
            Process::Open(child) => {
                let start = Instant::now();
                while start.elapsed() < TIMEOUT {
                    if let Some(status) = child.0.try_wait().map_err(|error| {
                        format!(
                            "{} WaitForSingleObject (unconfined child): {error}",
                            self.context
                        )
                    })? {
                        return status.code().map(i32::cast_unsigned).ok_or_else(|| {
                            format!(
                                "{} GetExitCodeProcess (unconfined child): exit code unavailable",
                                self.context
                            )
                        });
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(format!(
                    "{} WaitForSingleObject (unconfined child): Win32 {WAIT_TIMEOUT}",
                    self.context
                ))
            }
        }
    }
}

#[derive(Debug)]
struct Report {
    api: String,
    code: u32,
}

impl Report {
    fn parse(context: &str, bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| format!("{context} ReadFile (fixture report) Win32 0: {error}"))?;
        let (api, code) = text.trim_end().split_once('\t').ok_or_else(|| {
            format!("{context} ReadFile (fixture report) Win32 0: malformed report {text:?}")
        })?;
        Ok(Self {
            api: api.into(),
            code: code.parse().map_err(|error| {
                format!(
                    "{context} ReadFile (fixture reply) Win32 0: invalid reported code: {error}"
                )
            })?,
        })
    }

    fn expect(&self, apis: &[&str], codes: &[u32], step: &str) {
        assert!(
            apis.contains(&self.api.as_str()) && codes.contains(&self.code),
            "{step}: {} returned Win32 {} ({:#010x}), expected API {apis:?} and code {codes:?}",
            self.api,
            self.code,
            self.code,
        );
    }
}

struct Sid(PSID);

impl Sid {
    fn derive() -> Self {
        let name = wide(SPIKE_CONTAINER_NAME);
        let mut sid = ptr::null_mut();
        // SAFETY: the name is terminated and the SID out pointer is writable.
        let hr = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &raw mut sid) };
        assert!(
            hr >= 0 && !sid.is_null(),
            "DeriveAppContainerSidFromAppContainerName (fixture ACL): HRESULT {:#010x}",
            hr.cast_unsigned()
        );
        Self(sid)
    }

    fn text(&self) -> OsString {
        let mut buffer = ptr::null_mut();
        // SAFETY: the derived SID is valid and the pointer is writable.
        let ok = unsafe { ConvertSidToStringSidW(self.0, &raw mut buffer) };
        assert_ne!(
            ok,
            0,
            "ConvertSidToStringSidW: {}",
            io::Error::last_os_error()
        );
        // SAFETY: success returns a NUL-terminated LocalAlloc string.
        let text = unsafe { copy_wide(buffer) };
        // SAFETY: success transfers this allocation.
        unsafe { LocalFree(buffer.cast()) };
        text
    }
}

impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: the API allocated this SID and this owner frees it once.
        unsafe { FreeSid(self.0) };
    }
}

struct Local(*mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: the security API returned this LocalAlloc pointer, and null is accepted.
        unsafe { LocalFree(self.0) };
    }
}

fn grant_image_access(directory: &Path, sid: &Sid) {
    let path = wide(directory);
    let mut old_acl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: path identifies only the just-created directory. The outputs are writable.
    let code = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut old_acl,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    let _descriptor = Local(descriptor);
    assert_eq!(
        code, 0,
        "GetNamedSecurityInfoW (image directory DACL): Win32 {code}"
    );
    assert!(
        !old_acl.is_null(),
        "GetNamedSecurityInfoW returned a null DACL"
    );
    let mut ace = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        ..Default::default()
    };
    ace.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    ace.Trustee.TrusteeType = TRUSTEE_IS_USER;
    ace.Trustee.ptstrName = sid.0.cast();
    let mut new_acl = ptr::null_mut();
    // SAFETY: the original ACL and SID remain live and new_acl is writable.
    let code = unsafe { SetEntriesInAclW(1, &raw const ace, old_acl, &raw mut new_acl) };
    let _merged = Local(new_acl.cast());
    assert_eq!(
        code, 0,
        "SetEntriesInAclW (derived SID read/execute): Win32 {code}"
    );
    // SAFETY: this writes only the merged DACL on the isolated image directory.
    let code = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            new_acl,
            ptr::null_mut(),
        )
    };
    assert_eq!(
        code, 0,
        "SetNamedSecurityInfoW (fixture image directory only): Win32 {code}"
    );
}

fn container_storage(sid: &OsStr) -> Option<PathBuf> {
    let sid = wide(sid);
    let mut buffer = ptr::null_mut();
    // SAFETY: sid is terminated and buffer receives the query's allocated path.
    let status = unsafe { GetAppContainerFolderPath(sid.as_ptr(), &raw mut buffer) };
    if status < 0 {
        let hr = status.cast_unsigned();
        let code = hr & 0xffff;
        assert!(
            hr >> 16 == 0x8007
                && matches!(
                    code,
                    ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_NOT_FOUND
                ),
            "GetAppContainerFolderPath (unregistered LPAC SID): HRESULT {hr:#010x}, Win32 {code}"
        );
        eprintln!(
            "GetAppContainerFolderPath (unregistered LPAC SID): HRESULT {hr:#010x}, Win32 {code}"
        );
        return None;
    }
    assert!(
        !buffer.is_null(),
        "GetAppContainerFolderPath returned a null path"
    );
    // SAFETY: success returns a terminated string allocated by the COM task allocator.
    let path = PathBuf::from(unsafe { copy_wide(buffer) });
    // SAFETY: the caller owns this allocation.
    unsafe { CoTaskMemFree(buffer.cast()) };
    assert!(
        path.is_absolute(),
        "GetAppContainerFolderPath returned {}",
        path.display()
    );
    Some(path)
}

fn check_storage_tree(fixture: &Fixture, path: &Path, checked: &mut BTreeSet<PathBuf>) {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => panic!(
            "GetFileInformationByHandle (storage {}): {error}",
            path.display()
        ),
    };
    assert_eq!(
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT,
        0,
        "unexpected reparse point in LPAC storage at {}",
        path.display()
    );
    assert!(
        metadata.is_dir(),
        "LPAC storage is not a directory at {}",
        path.display()
    );
    if !checked.insert(path.into()) {
        return;
    }
    fixture.refuse_writes(path);
    for entry in fs::read_dir(path)
        .unwrap_or_else(|e| panic!("FindFirstFileW (LPAC storage {}): {e}", path.display()))
    {
        let entry = entry.expect("FindNextFileW (LPAC storage)");
        let kind = entry
            .file_type()
            .expect("GetFileAttributesW (LPAC storage entry)");
        if kind.is_dir() || kind.is_symlink() {
            check_storage_tree(fixture, &entry.path(), checked);
        }
    }
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain([0]).collect()
}

unsafe fn copy_wide(value: *const u16) -> OsString {
    let mut length = 0;
    // SAFETY: callers pass a live NUL-terminated string from a Windows allocation.
    unsafe {
        while *value.add(length) != 0 {
            length += 1;
        }
        OsString::from_wide(std::slice::from_raw_parts(value, length))
    }
}
