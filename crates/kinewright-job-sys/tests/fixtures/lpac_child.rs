#![cfg_attr(windows, allow(unsafe_code))]

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    match child::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("LPAC fixture before reply: {error}");
            std::process::ExitCode::from(71)
        }
    }
}

#[cfg(windows)]
mod child {
    use std::{
        env,
        ffi::{OsStr, OsString},
        fs::{File, OpenOptions},
        io::{self, Read, Write},
        net::{SocketAddrV4, TcpStream},
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
        },
        path::Path,
        ptr,
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, GetLastError, WAIT_FAILED,
            WAIT_OBJECT_0,
        },
        System::{
            Console::GetConsoleCP,
            Memory::{
                MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
            },
            Registry::{HKEY_CURRENT_USER, KEY_WRITE, RegCloseKey, RegCreateKeyExW, RegDeleteKeyW},
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, GetCurrentProcess, OpenProcess,
                PROCESS_DUP_HANDLE, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
                STARTUPINFOW, TerminateProcess, WaitForSingleObject,
            },
        },
    };

    const SOURCE: &[u8] = b"LPAC source bytes";

    pub fn run() -> io::Result<()> {
        let mut args = env::args_os().skip(1);
        let file = next_handle(&mut args)?;
        let request = next_handle(&mut args)?;
        let reply = next_handle(&mut args)?;
        let case = next_arg(&mut args)?;
        let target = next_arg(&mut args)?;

        if request.is_null() && reply.is_null() && file.is_null() {
            return exercise(
                &case,
                &target,
                None,
                &mut io::stdin().lock(),
                &mut io::stdout().lock(),
            );
        }

        let source = duplicate_file(file)?;
        let mut request = duplicate_file(request)?;
        let mut reply = duplicate_file(reply)?;
        exercise(&case, &target, Some(source), &mut request, &mut reply)
    }

    fn duplicate_file(raw: RawHandle) -> io::Result<File> {
        let mut copy = ptr::null_mut();
        // SAFETY: Windows validates the input handle value. Success transfers a distinct handle.
        let ok = unsafe {
            let current = GetCurrentProcess();
            DuplicateHandle(
                current,
                raw,
                current,
                &raw mut copy,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if ok == 0 {
            return Err(io::Error::other(format!(
                "DuplicateHandle (fixture inherited handle): {}",
                io::Error::last_os_error()
            )));
        }
        // SAFETY: success returned an owned handle and no other owner exists.
        Ok(unsafe { File::from_raw_handle(copy) })
    }

    fn next_arg(args: &mut impl Iterator<Item = OsString>) -> io::Result<OsString> {
        args.next()
            .ok_or_else(|| io::Error::other("missing fixture argument"))
    }

    fn next_handle(args: &mut impl Iterator<Item = OsString>) -> io::Result<RawHandle> {
        next_arg(args)?
            .to_str()
            .and_then(|value| value.parse::<usize>().ok())
            .map(|value| value as RawHandle)
            .ok_or_else(|| io::Error::other("invalid fixture handle"))
    }

    fn exercise(
        case: &OsStr,
        target: &OsStr,
        source: Option<File>,
        input: &mut impl Read,
        output: &mut impl Write,
    ) -> io::Result<()> {
        let mut start = [0u8];
        input
            .read_exact(&mut start)
            .map_err(|e| io::Error::other(format!("ReadFile (request): {e}")))?;
        if start != *b"!" {
            return Err(io::Error::other("ReadFile (request): wrong handshake byte"));
        }
        let (api, code) = match case.to_str() {
            Some("ready") => ready(target),
            Some("source") => {
                read_source(source.ok_or_else(|| io::Error::other("missing source handle"))?)
            }
            Some("create") => file_write(target, "create"),
            Some("grow") => file_write(target, "grow"),
            Some("mutate") => file_write(target, "mutate"),
            Some("registry") => registry(target),
            Some("connect") => {
                let address: SocketAddrV4 = parse(target)?;
                (
                    "connect",
                    result_code(TcpStream::connect_timeout(
                        &address.into(),
                        Duration::from_secs(2),
                    )),
                )
            }
            Some("process") => (
                "OpenProcess(PROCESS_DUP_HANDLE)",
                open_process(parse(target)?, PROCESS_DUP_HANDLE),
            ),
            Some("process-query") => (
                "OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)",
                open_process(parse(target)?, PROCESS_QUERY_LIMITED_INFORMATION),
            ),
            Some("pipe") => (
                "CreateFileW(named pipe)",
                result_code(OpenOptions::new().write(true).open(target)),
            ),
            Some("spawn") => spawn(target),
            Some("memory") => ("VirtualAlloc", allocate(parse(target)?)),
            Some("hold") => ("ReadFile(hold)", 0),
            _ => {
                return Err(io::Error::other(format!(
                    "unknown fixture case {}",
                    case.display()
                )));
            }
        };
        let report = format!("{api}\t{code}\n");
        output.write_all(report.as_bytes())?;
        output.flush()?;
        if case == "hold" || case == "memory" {
            // The host must kill a waiting child. EOF is a distinct fixture failure.
            input.read_exact(&mut start)?;
        }
        Ok(())
    }

    fn parse<T: std::str::FromStr>(value: &OsStr) -> io::Result<T> {
        value
            .to_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| io::Error::other("invalid fixture argument"))
    }

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain([0]).collect()
    }

    fn result_code<T>(result: io::Result<T>) -> u32 {
        match result {
            Ok(_) => 0,
            Err(error) => error.raw_os_error().unwrap_or(-1).cast_unsigned(),
        }
    }

    fn ready(expected_dir: &OsStr) -> (&'static str, u32) {
        let actual = match env::current_dir() {
            Ok(path) => path,
            Err(error) => return ("GetCurrentDirectoryW", result_code::<()>(Err(error))),
        };
        if actual != Path::new(expected_dir) {
            return ("GetCurrentDirectoryW(cwd mismatch)", 0);
        }
        let mut vars = env::vars_os();
        if !matches!(vars.next(), Some((key, value)) if key.eq_ignore_ascii_case("SystemRoot") && Path::new(&value).is_absolute())
            || vars.next().is_some()
        {
            return ("GetEnvironmentStringsW(expected only SystemRoot)", 0);
        }
        // SAFETY: this query has no pointer or handle argument.
        if unsafe { GetConsoleCP() } != 0 {
            return ("GetConsoleCP(expected no console)", 0);
        }
        ("ready", 0)
    }

    fn read_source(mut source: File) -> (&'static str, u32) {
        let mut bytes = [0; SOURCE.len()];
        if let Err(error) = source.read_exact(&mut bytes) {
            return ("ReadFile(source)", result_code::<()>(Err(error)));
        }
        if bytes != SOURCE {
            return ("ReadFile(source bytes mismatch)", 0);
        }
        (
            "WriteFile(read-only inherited source)",
            result_code(source.write_all(b"unexpected")),
        )
    }

    fn file_write(target: &OsStr, mode: &str) -> (&'static str, u32) {
        let mut options = OpenOptions::new();
        let result = match mode {
            "create" => options.write(true).create_new(true).open(target),
            "grow" => options.append(true).open(target),
            _ => options.write(true).open(target),
        };
        match result {
            Ok(mut file) => ("WriteFile(canary)", result_code(file.write_all(b"canary"))),
            Err(error) => ("CreateFileW(canary)", result_code::<()>(Err(error))),
        }
    }

    fn registry(target: &OsStr) -> (&'static str, u32) {
        let key_name = wide(target);
        let mut key = ptr::null_mut();
        // SAFETY: the name is terminated, key is writable and HKCU is predefined.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key_name.as_ptr(),
                0,
                ptr::null(),
                0,
                KEY_WRITE,
                ptr::null(),
                &raw mut key,
                ptr::null_mut(),
            )
        };
        if status == 0 {
            // SAFETY: key was opened successfully. Delete only the unique canary key.
            let (close, delete) = unsafe {
                (
                    RegCloseKey(key),
                    RegDeleteKeyW(HKEY_CURRENT_USER, key_name.as_ptr()),
                )
            };
            if close != 0 {
                return ("RegCloseKey", close);
            }
            if delete != 0 {
                return ("RegDeleteKeyW", delete);
            }
        }
        ("RegCreateKeyExW(HKCU)", status)
    }

    fn open_process(pid: u32, access: u32) -> u32 {
        // SAFETY: this call requests only a local process handle, which success transfers.
        let process = unsafe { OpenProcess(access, 0, pid) };
        if process.is_null() {
            // SAFETY: read this thread's last error immediately after a failed call.
            unsafe { GetLastError() }
        } else {
            // SAFETY: this is the owned handle returned by OpenProcess.
            unsafe { CloseHandle(process) };
            0
        }
    }

    fn spawn(program: &OsStr) -> (&'static str, u32) {
        let program = wide(program);
        let startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>()
                .try_into()
                .expect("STARTUPINFOW size"),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: the program is terminated, startup is initialized and info is writable.
        let ok = unsafe {
            CreateProcessW(
                program.as_ptr(),
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                0,
                CREATE_NO_WINDOW | CREATE_SUSPENDED,
                ptr::null(),
                ptr::null(),
                &raw const startup,
                &raw mut info,
            )
        };
        if ok == 0 {
            // SAFETY: read this thread's last error immediately after a failed call.
            return ("CreateProcessW(second process)", unsafe { GetLastError() });
        }
        // SAFETY: CreateProcessW has transferred these handles to this process.
        let (process, _thread) = unsafe {
            (
                OwnedHandle::from_raw_handle(info.hProcess),
                OwnedHandle::from_raw_handle(info.hThread),
            )
        };
        // SAFETY: the child is still suspended and this process owns the process handle.
        if unsafe { TerminateProcess(process.as_raw_handle(), 1) } == 0 {
            // SAFETY: read this thread's last error immediately after a failed call.
            return ("TerminateProcess(second process)", unsafe {
                GetLastError()
            });
        }
        // SAFETY: the owned process handle remains live until the wait completes.
        let status = unsafe { WaitForSingleObject(process.as_raw_handle(), 2_000) };
        if status == WAIT_FAILED {
            // SAFETY: read this thread's last error immediately after a failed call.
            return ("WaitForSingleObject(second process) failed", unsafe {
                GetLastError()
            });
        }
        if status != WAIT_OBJECT_0 {
            return ("WaitForSingleObject(second process)", status);
        }
        ("CreateProcessW(second process)", 0)
    }

    fn allocate(bytes: usize) -> u32 {
        // SAFETY: null address lets Windows select a range. No memory is dereferenced.
        let memory =
            unsafe { VirtualAlloc(ptr::null(), bytes, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
        if memory.is_null() {
            // SAFETY: read this thread's last error immediately after a failed call.
            unsafe { GetLastError() }
        } else {
            // SAFETY: memory is the base of the owned reservation. MEM_RELEASE requires zero size.
            unsafe { VirtualFree(memory, 0, MEM_RELEASE) };
            0
        }
    }
}
