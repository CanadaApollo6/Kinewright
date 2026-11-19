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
        net::SocketAddrV4,
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
        },
        path::Path,
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, GetLastError,
            INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0,
        },
        Networking::WinSock::{
            AF_INET, IN_ADDR, IN_ADDR_0, INVALID_SOCKET, IPPROTO_TCP, SOCK_STREAM, SOCKADDR_IN,
            SOCKET, WSACleanup, WSADATA, WSAGetLastError, WSAStartup, closesocket, connect, socket,
        },
        Storage::FileSystem::WriteFile,
        System::{
            Console::{
                GetConsoleWindow, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
                STD_OUTPUT_HANDLE,
            },
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
        let stdio = request.is_null() && reply.is_null() && file.is_null();
        let panic_reply = if stdio {
            // SAFETY: this returns the unconfined control's redirected stdout pipe.
            unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }
        } else {
            reply
        };
        install_panic_report(panic_reply as usize);
        let case = next_arg(&mut args)?;
        let target = next_arg(&mut args)?;

        if stdio {
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

    fn install_panic_report(reply: usize) {
        std::panic::set_hook(Box::new(move |info| {
            let message = format!("{info}").replace(['\r', '\n', '\t'], " ");
            let mut report = format!("panic\t101\t{message}");
            report.truncate(report.floor_char_boundary(4095));
            report.push('\n');
            let mut written = 0;
            // SAFETY: the original reply handle remains open for the life of this process.
            // Write directly so a panic during a stdio write never reacquires its Rust lock.
            unsafe {
                WriteFile(
                    reply as RawHandle,
                    report.as_ptr(),
                    u32::try_from(report.len()).unwrap_or(4096),
                    &raw mut written,
                    ptr::null_mut(),
                );
            }
        }));
    }

    struct Winsock;
    impl Drop for Winsock {
        fn drop(&mut self) {
            // SAFETY: this guard corresponds to a successful WSAStartup.
            unsafe { WSACleanup() };
        }
    }

    struct Socket(SOCKET);
    impl Drop for Socket {
        fn drop(&mut self) {
            // SAFETY: the successful socket call gave this guard an owned socket.
            unsafe { closesocket(self.0) };
        }
    }

    fn connect_loopback(address: SocketAddrV4, output: &mut impl Write) -> io::Result<()> {
        if !address.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "non-loopback address",
            ));
        }
        let mut data = WSADATA::default();
        // SAFETY: data is writable and 0x0202 requests Winsock 2.2.
        let startup = unsafe { WSAStartup(0x0202, &raw mut data) };
        if startup != 0 {
            return writeln!(output, "WSAStartup\t{startup}");
        }
        let _winsock = Winsock;
        // SAFETY: Winsock initialized successfully and the socket type is TCP/IPv4.
        let raw = unsafe { socket(i32::from(AF_INET), SOCK_STREAM, IPPROTO_TCP) };
        if raw == INVALID_SOCKET {
            // SAFETY: read the thread's Winsock error immediately after the failed call.
            let code = unsafe { WSAGetLastError() };
            return writeln!(output, "socket\t{code}\tWSAStartup=0");
        }
        let socket = Socket(raw);
        let endpoint = SOCKADDR_IN {
            sin_family: AF_INET,
            sin_port: address.port().to_be(),
            sin_addr: IN_ADDR {
                S_un: IN_ADDR_0 {
                    S_addr: u32::from_ne_bytes(address.ip().octets()),
                },
            },
            sin_zero: [0; 8],
        };
        // SAFETY: socket and the encoded IPv4 endpoint remain live through the call.
        let result = unsafe {
            connect(
                socket.0,
                (&raw const endpoint).cast(),
                i32::try_from(size_of::<SOCKADDR_IN>()).expect("IPv4 address size"),
            )
        };
        // SAFETY: Winsock records the error on the current thread.
        let code = if result == 0 {
            0
        } else {
            unsafe { WSAGetLastError() }
        };
        writeln!(output, "connect\t{code}\tWSAStartup=0 socket=0")
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
        #[cfg(feature = "lpac-diagnostics")]
        match case.to_str() {
            Some("profile-registry") => return super::profile_registry::report(target, output),

            Some(
                "profile-registry-grow" | "profile-registry-change" | "profile-registry-value",
            ) => {
                return super::profile_registry::write_value(case, target, output);
            }
            Some("profile-environment") => return super::profile_permissions::environment(output),
            Some("profile-objects") => return super::profile_permissions::objects(target, output),
            Some("profile-registry-direct") => {
                return super::profile_registry::write_direct(target, output);
            }
            _ => {}
        }
        let (api, code) = match case.to_str() {
            Some("ready") => ready(target, source.is_some(), true),
            #[cfg(feature = "lpac-diagnostics")]
            Some("package-only-read") => match File::open(target) {
                Ok(mut file) => {
                    let mut bytes = Vec::new();
                    match file.read_to_end(&mut bytes) {
                        Ok(_) if bytes == b"packages-only" => ("ReadFile(package-only canary)", 0),
                        Ok(_) => ("ReadFile(package-only canary bytes mismatch)", 0),
                        Err(error) => (
                            "ReadFile(package-only canary)",
                            result_code::<()>(Err(error)),
                        ),
                    }
                }
                Err(error) => (
                    "CreateFileW(package-only canary)",
                    result_code::<()>(Err(error)),
                ),
            },
            #[cfg(feature = "lpac-diagnostics")]
            Some("ready-parent-environment") => ready(target, source.is_some(), false),
            Some("source") => {
                read_source(source.ok_or_else(|| io::Error::other("missing source handle"))?)
            }
            Some("create") => file_write(target, "create"),
            Some("grow") => file_write(target, "grow"),
            Some("mutate") => file_write(target, "mutate"),
            Some("registry") => registry(target),
            Some("connect") => return connect_loopback(parse(target)?, output),
            #[cfg(feature = "lpac-diagnostics")]
            Some("panic-witness") => panic!("intentional fixture panic witness"),
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

    fn ready(expected_dir: &OsStr, lpac: bool, minimal_environment: bool) -> (&'static str, u32) {
        let actual = match env::current_dir() {
            Ok(path) => path,
            Err(error) => return ("GetCurrentDirectoryW", result_code::<()>(Err(error))),
        };
        if actual != Path::new(expected_dir) {
            return ("GetCurrentDirectoryW(cwd mismatch)", 0);
        }
        let system_root =
            env::var_os("SystemRoot").is_some_and(|value| Path::new(&value).is_absolute());
        if !system_root || (minimal_environment && env::vars_os().count() != 1) {
            return ("GetEnvironmentStringsW(unexpected startup environment)", 0);
        }
        // SAFETY: this query has no arguments. A pseudoconsole also returns a window handle.
        if !unsafe { GetConsoleWindow() }.is_null() {
            return ("GetConsoleWindow(unexpected console association)", 0);
        }
        if lpac {
            // The LPAC protocol uses explicit handles. Only the control redirects standard I/O.
            for (slot, step) in [
                (STD_INPUT_HANDLE, "GetStdHandle(LPAC stdin must be absent)"),
                (
                    STD_OUTPUT_HANDLE,
                    "GetStdHandle(LPAC stdout must be absent)",
                ),
                (STD_ERROR_HANDLE, "GetStdHandle(LPAC stderr must be absent)"),
            ] {
                // SAFETY: slot names one of the three standard-handle slots.
                let handle = unsafe { GetStdHandle(slot) };
                if handle == INVALID_HANDLE_VALUE {
                    // SAFETY: read this thread's last error immediately after the failed query.
                    return (step, unsafe { GetLastError() });
                }
                if !handle.is_null() {
                    return (step, 0);
                }
            }
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

#[cfg(all(windows, feature = "lpac-diagnostics"))]
#[path = "lpac_registry_key.rs"]
mod registry_key;

#[cfg(all(windows, feature = "lpac-diagnostics"))]
mod profile_registry {
    use std::{
        ffi::OsStr,
        io::{self, Write},
        mem::size_of_val,
        os::windows::ffi::OsStrExt,
        ptr,
    };
    use windows_sys::{
        Wdk::System::Registry::{KeyNameInformation, NtQueryKey},
        Win32::{
            Foundation::ERROR_INVALID_DATA,
            Security::Isolation::GetAppContainerRegistryLocation,
            System::Registry::{
                HKEY, KEY_CREATE_SUB_KEY, KEY_READ, KEY_SET_VALUE, KEY_WRITE, REG_BINARY,
                RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegSetValueExW,
            },
        },
    };

    pub(super) struct Key(pub(super) HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the API transferred ownership of this key handle.
            unsafe { RegCloseKey(self.0) };
        }
    }

    pub(super) fn open(access: u32) -> Result<Key, i32> {
        let mut root = ptr::null_mut();
        // SAFETY: root is a writable out parameter for the current AppContainer's registry.
        let status = unsafe { GetAppContainerRegistryLocation(access, &raw mut root) };
        if status < 0 {
            return Err(status);
        }
        if root.is_null() {
            return Err(0x8000_ffff_u32.cast_signed());
        }
        Ok(Key(root))
    }

    fn key_name(key: &Key) -> String {
        let mut data = [0u32; 512];
        let mut returned = 0;
        // SAFETY: the live key is borrowed, the aligned output is writable, and the length
        // matches the buffer. KEY_NAME_INFORMATION starts with a DWORD byte count.
        let status = unsafe {
            NtQueryKey(
                key.0,
                KeyNameInformation,
                data.as_mut_ptr().cast(),
                size_of_val(&data).try_into().expect("bounded name buffer"),
                &raw mut returned,
            )
        };
        if status < 0 {
            return format!(
                "NtQueryKey(KeyNameInformation) NTSTATUS {:#010x}",
                status.cast_unsigned()
            );
        }
        let bytes = data[0] as usize;
        if bytes == 0
            || !bytes.is_multiple_of(2)
            || bytes + 4 > (returned as usize).min(size_of_val(&data))
        {
            return format!("NtQueryKey(KeyNameInformation) Win32 {ERROR_INVALID_DATA}");
        }
        // SAFETY: successful NtQueryKey wrote bytes of UTF-16 after the DWORD header.
        let name =
            unsafe { std::slice::from_raw_parts(data.as_ptr().add(1).cast::<u16>(), bytes / 2) };
        String::from_utf16_lossy(name)
    }

    pub fn report(target: &OsStr, output: &mut impl Write) -> io::Result<()> {
        let read = open(KEY_READ);
        let writable = open(KEY_CREATE_SUB_KEY);
        let name = match (&read, &writable) {
            (Ok(key), _) | (Err(_), Ok(key)) => key_name(key),
            (Err(status), Err(_)) => format!(
                "GetAppContainerRegistryLocation(KEY_READ) HRESULT {:#010x}",
                status.cast_unsigned()
            ),
        };
        let (step, code) = match writable {
            Ok(root) => create_canary(&root, target),
            Err(status) => (
                "GetAppContainerRegistryLocation(KEY_CREATE_SUB_KEY) HRESULT",
                status.cast_unsigned(),
            ),
        };
        let message = format!("{step}\t{code}\t{name}\n");
        output.write_all(message.as_bytes())?;
        output.flush()
    }

    pub fn write_value(case: &OsStr, target: &OsStr, output: &mut impl Write) -> io::Result<()> {
        let root = match open(KEY_SET_VALUE) {
            Ok(root) => root,
            Err(status) => {
                return writeln!(
                    output,
                    "GetAppContainerRegistryLocation(KEY_SET_VALUE) HRESULT\t{}",
                    status.cast_unsigned()
                );
            }
        };
        let name: Vec<u16> = target.encode_wide().chain([0]).collect();
        let bytes: &[u8] = if case == "profile-registry-grow" {
            b"profile canary enlarged"
        } else {
            b"changed!"
        };
        // SAFETY: root is owned, name terminated, and bytes holds the specified data.
        let status = unsafe {
            RegSetValueExW(
                root.0,
                name.as_ptr(),
                0,
                REG_BINARY,
                bytes.as_ptr(),
                u32::try_from(bytes.len()).expect("canary length"),
            )
        };
        writeln!(output, "RegSetValueExW(profile value)\t{status}")
    }

    pub fn write_direct(target: &OsStr, output: &mut impl Write) -> io::Result<()> {
        let target = target
            .to_str()
            .ok_or_else(|| io::Error::other("registry target is not Unicode"))?;
        let (key, name) = target
            .split_once('\t')
            .ok_or_else(|| io::Error::other("missing registry canary name"))?;
        let key = match super::registry_key::open_registry(key, KEY_SET_VALUE) {
            Ok(key) => key,
            Err(error) => {
                return writeln!(output, "NtOpenKey(KEY_SET_VALUE)\t{}\t{error}", error.code);
            }
        };
        let name: Vec<u16> = OsStr::new(name).encode_wide().chain([0]).collect();
        // SAFETY: key is live, name is terminated and the supplied data contains six bytes.
        let code =
            unsafe { RegSetValueExW(key.0, name.as_ptr(), 0, REG_BINARY, b"direct".as_ptr(), 6) };
        writeln!(
            output,
            "RegSetValueExW(profile direct)\t{code}\tNtOpenKey(KEY_SET_VALUE)=0"
        )
    }

    fn create_canary(root: &Key, target: &OsStr) -> (&'static str, u32) {
        let target: Vec<u16> = target.encode_wide().chain([0]).collect();
        let mut key = ptr::null_mut();
        // SAFETY: root is owned, target is a unique terminated child name, and key is writable.
        let status = unsafe {
            RegCreateKeyExW(
                root.0,
                target.as_ptr(),
                0,
                ptr::null(),
                0,
                KEY_WRITE,
                ptr::null(),
                &raw mut key,
                ptr::null_mut(),
            )
        };
        if status != 0 {
            return ("RegCreateKeyExW(profile canary)", status);
        }
        drop(Key(key));
        // SAFETY: target names the canary just created relative to this exact profile root.
        let cleanup = unsafe { RegDeleteKeyW(root.0, target.as_ptr()) };
        if cleanup != 0 {
            return ("RegCreateKeyExW(profile canary)=0; RegDeleteKeyW", cleanup);
        }
        ("RegCreateKeyExW(profile canary)", 0)
    }
}

#[cfg(all(windows, feature = "lpac-diagnostics"))]
#[path = "profile_permissions.rs"]
mod profile_permissions;
