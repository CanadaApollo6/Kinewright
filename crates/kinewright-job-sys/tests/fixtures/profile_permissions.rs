use std::{
    ffi::OsStr,
    io::{self, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    ptr,
};
use windows_sys::Win32::{
    Foundation::GetLastError,
    Security::{Isolation::GetAppContainerNamedObjectPath, TOKEN_QUERY},
    System::Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken, SetEvent},
};

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

fn last_error() -> u32 {
    // SAFETY: read this thread's last error after a failed Win32 call.
    unsafe { GetLastError() }
}

fn token() -> Result<OwnedHandle, u32> {
    let mut token = ptr::null_mut();
    // SAFETY: the self-process pseudohandle is valid and token is writable.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(last_error());
    }
    // SAFETY: the call transferred ownership of this token handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}

pub(super) fn objects(target: &OsStr, output: &mut impl Write) -> io::Result<()> {
    let token = match token() {
        Ok(token) => token,
        Err(code) => return writeln!(output, "OpenProcessToken(object namespace)\t{code}"),
    };
    let mut path = [0u16; 1024];
    let mut length = 0;
    // SAFETY: the token is live and the UTF-16 output and length pointer are writable.
    let ok = unsafe {
        GetAppContainerNamedObjectPath(
            token.as_raw_handle(),
            ptr::null_mut(),
            1024,
            path.as_mut_ptr(),
            &raw mut length,
        )
    };
    let location = if ok == 0 {
        format!("GetAppContainerNamedObjectPath Win32 {}", last_error())
    } else {
        let end = path
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(path.len());
        String::from_utf16_lossy(&path[..end])
    };
    let name = wide(target);
    // SAFETY: the name is a unique terminated canary in the child's default object namespace.
    let handle = unsafe { CreateEventW(ptr::null(), 1, 0, name.as_ptr()) };
    let (api, code) = if handle.is_null() {
        ("CreateEventW(default AppContainer namespace)", last_error())
    } else {
        // SAFETY: the new event handle is owned by the fixture.
        let event = unsafe { OwnedHandle::from_raw_handle(handle) };
        // SAFETY: event is an owned live event handle.
        let set = unsafe { SetEvent(event.as_raw_handle()) };
        (
            "CreateEventW=0; SetEvent(AppContainer canary)",
            if set == 0 { last_error() } else { 0 },
        )
    };
    writeln!(
        output,
        "{api}\t{code}\tlocation={location:?}; name={}",
        target.display()
    )
}

pub(super) fn owner_access(target: &OsStr, output: &mut impl Write) -> io::Result<()> {
    use std::fmt::Write as _;
    use windows_sys::Win32::{
        Security::{EqualSid, GetTokenInformation, TOKEN_USER, TokenUser},
        Storage::FileSystem::{WRITE_DAC, WRITE_OWNER},
    };
    let fields: Vec<_> = target
        .to_str()
        .unwrap_or_default()
        .splitn(5, '\t')
        .collect();
    let [kind, right, owner, package, path] = fields.as_slice() else {
        return Err(io::Error::other("missing owner access witness arguments"));
    };
    let access = match *right {
        "WRITE_DAC" => WRITE_DAC,
        "WRITE_OWNER" => WRITE_OWNER,
        _ => return Err(io::Error::other("unknown owner access mask")),
    };
    let file = match *kind {
        "file" | "file-control" => true,
        "key" => false,
        _ => return Err(io::Error::other("unknown owner access object")),
    };
    let mask = if file { "FA" } else { "KA" };
    let owner_ace = if *kind == "file-control" {
        String::new()
    } else {
        format!("(A;;{mask};;;{owner})")
    };
    let sddl =
        format!("O:{owner}D:P(A;;{mask};;;SY)(A;;{mask};;;BA){owner_ace}(A;;{mask};;;{package})");
    let descriptor = match OwnerDescriptor::new(&sddl) {
        Ok(descriptor) => descriptor,
        Err((api, code)) => return writeln!(output, "{api}\t{code}"),
    };
    let token = match token() {
        Ok(token) => token,
        Err(code) => return writeln!(output, "OpenProcessToken(owner witness)\t{code}"),
    };
    let mut user = [0usize; 32];
    let mut length = 0;
    // SAFETY: token is live, and the fixed aligned buffer holds TOKEN_USER and a SID.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            user.as_mut_ptr().cast(),
            u32::try_from(size_of_val(&user)).expect("owner witness token buffer size"),
            &raw mut length,
        )
    };
    if ok == 0 {
        return writeln!(output, "GetTokenInformation(TokenUser)\t{}", last_error());
    }
    // SAFETY: the SID points into the live, initialized token buffer and the owner descriptor is live.
    let same = unsafe {
        EqualSid(
            (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid,
            descriptor.owner,
        )
    } != 0;
    let api = if file { "NtOpenFile" } else { "RegOpenKeyExW" };
    let handle = match SecurityObject::open(path, file, access) {
        Ok(handle) => handle,
        Err(code) => {
            return writeln!(
                output,
                "{api}({right})\t{code}\tTokenUser matches owner SID={same}"
            );
        }
    };
    let setter = handle.setter();
    let mut detail = format!("TokenUser matches owner SID={same}");
    if access == WRITE_OWNER {
        let owner_code = handle.set_owner(&descriptor);
        let dacl_code = handle.set_dacl(&descriptor);
        let followup = match SecurityObject::open(path, file, WRITE_DAC) {
            Ok(writable) => format!(
                "open WRITE_DAC=0 {setter}(DACL)={}",
                writable.set_dacl(&descriptor)
            ),
            Err(code) => format!("open WRITE_DAC={code}"),
        };
        write!(
            detail,
            " {setter}(OWNER)={owner_code} {setter}(DACL via WRITE_OWNER)={dacl_code} {followup}"
        )
        .expect("string formatting");
    } else {
        let dacl_code = handle.set_dacl(&descriptor);
        write!(detail, " {setter}(DACL)={dacl_code}").expect("string formatting");
    }
    writeln!(output, "{api}({right})\t0\t{detail}")
}

struct OwnerDescriptor {
    raw: windows_sys::Win32::Security::PSECURITY_DESCRIPTOR,
    owner: windows_sys::Win32::Security::PSID,
    dacl: *mut windows_sys::Win32::Security::ACL,
}

impl OwnerDescriptor {
    fn new(text: &str) -> Result<Self, (&'static str, u32)> {
        use windows_sys::Win32::Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
        };
        let mut descriptor = Self {
            raw: ptr::null_mut(),
            owner: ptr::null_mut(),
            dacl: ptr::null_mut(),
        };
        let text = wide(OsStr::new(text));
        // SAFETY: text is terminated and descriptor.raw is writable.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor.raw,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err((
                "ConvertStringSecurityDescriptorToSecurityDescriptorW(owner witness)",
                last_error(),
            ));
        }
        let (mut present, mut defaulted) = (0, 0);
        // SAFETY: the descriptor is live and both outputs are writable.
        if unsafe {
            GetSecurityDescriptorOwner(
                descriptor.raw,
                &raw mut descriptor.owner,
                &raw mut defaulted,
            )
        } == 0
        {
            return Err(("GetSecurityDescriptorOwner", last_error()));
        }
        // SAFETY: the descriptor is live and the three outputs are writable.
        if unsafe {
            GetSecurityDescriptorDacl(
                descriptor.raw,
                &raw mut present,
                &raw mut descriptor.dacl,
                &raw mut defaulted,
            )
        } == 0
        {
            return Err(("GetSecurityDescriptorDacl", last_error()));
        }
        if descriptor.owner.is_null() || present == 0 || descriptor.dacl.is_null() {
            return Err((
                "GetSecurityDescriptorOwner/GetSecurityDescriptorDacl",
                windows_sys::Win32::Foundation::ERROR_INVALID_DATA,
            ));
        }
        Ok(descriptor)
    }
}

impl Drop for OwnerDescriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor owns this LocalAlloc allocation, or null on failure.
        unsafe { windows_sys::Win32::Foundation::LocalFree(self.raw) };
    }
}

struct SecurityObject {
    handle: windows_sys::Win32::Foundation::HANDLE,
    file: bool,
}

impl SecurityObject {
    fn open(path: &str, file: bool, access: u32) -> Result<Self, u32> {
        use windows_sys::Win32::System::Registry::{HKEY_USERS, RegOpenKeyExW};
        let handle = if file {
            open_file_for_owner_probe(path, access)?
        } else {
            let path = wide(OsStr::new(path));
            let mut handle = ptr::null_mut();
            // SAFETY: path is terminated and names the test user's storage key.
            let code =
                unsafe { RegOpenKeyExW(HKEY_USERS, path.as_ptr(), 0, access, &raw mut handle) };
            if code != 0 {
                return Err(code);
            }
            handle
        };
        Ok(Self { handle, file })
    }

    fn setter(&self) -> &'static str {
        if self.file {
            "NtSetSecurityObject"
        } else {
            "SetSecurityInfo"
        }
    }

    fn set_file_security(&self, information: u32, descriptor: &OwnerDescriptor) -> u32 {
        use windows_sys::{
            Wdk::Storage::FileSystem::NtSetSecurityObject, Win32::Foundation::RtlNtStatusToDosError,
        };
        // SAFETY: the file handle and self-relative descriptor are live during the call.
        let status = unsafe { NtSetSecurityObject(self.handle, information, descriptor.raw) };
        if status < 0 {
            // SAFETY: translates the status code without accessing caller memory.
            unsafe { RtlNtStatusToDosError(status) }
        } else {
            0
        }
    }

    fn set_dacl(&self, descriptor: &OwnerDescriptor) -> u32 {
        use windows_sys::Win32::Security::{
            Authorization::{SE_REGISTRY_KEY, SetSecurityInfo},
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        };
        if self.file {
            // The D:P in the self-relative descriptor carries the protected DACL.
            return self.set_file_security(DACL_SECURITY_INFORMATION, descriptor);
        }
        // SAFETY: this is the fixture handle and its explicit canary DACL stays live for the write.
        unsafe {
            SetSecurityInfo(
                self.handle,
                SE_REGISTRY_KEY,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                descriptor.dacl,
                ptr::null_mut(),
            )
        }
    }

    fn set_owner(&self, descriptor: &OwnerDescriptor) -> u32 {
        use windows_sys::Win32::Security::{
            Authorization::{SE_REGISTRY_KEY, SetSecurityInfo},
            OWNER_SECURITY_INFORMATION,
        };
        if self.file {
            return self.set_file_security(OWNER_SECURITY_INFORMATION, descriptor);
        }
        // SAFETY: this is the fixture handle and the owner SID remains live.
        unsafe {
            SetSecurityInfo(
                self.handle,
                SE_REGISTRY_KEY,
                OWNER_SECURITY_INFORMATION,
                descriptor.owner,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        }
    }
}

fn open_file_for_owner_probe(
    path: &str,
    access: u32,
) -> Result<windows_sys::Win32::Foundation::HANDLE, u32> {
    use windows_sys::{
        Wdk::{
            Foundation::OBJECT_ATTRIBUTES,
            Storage::FileSystem::{FILE_OPEN_REPARSE_POINT, NtOpenFile},
        },
        Win32::{
            Foundation::{
                ERROR_FILENAME_EXCED_RANGE, OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError,
                UNICODE_STRING,
            },
            Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE},
            System::IO::IO_STATUS_BLOCK,
        },
    };
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let mut name: Vec<u16> = format!(r"\??\{path}").encode_utf16().collect();
    let length =
        u16::try_from(size_of_val(name.as_slice())).map_err(|_| ERROR_FILENAME_EXCED_RANGE)?;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>()).expect("object attributes size"),
        ObjectName: &raw const name,
        Attributes: OBJ_CASE_INSENSITIVE,
        ..Default::default()
    };
    let mut status_block = IO_STATUS_BLOCK::default();
    let mut handle = ptr::null_mut();
    // SAFETY: the counted UTF-16 name, attributes and outputs remain live until the call returns.
    // No synchronous I/O option adds SYNCHRONIZE to the requested descriptor right.
    let status = unsafe {
        NtOpenFile(
            &raw mut handle,
            access,
            &raw const attributes,
            &raw mut status_block,
            FILE_SHARE_DELETE | FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN_REPARSE_POINT,
        )
    };
    if status < 0 {
        // SAFETY: status is the NTSTATUS from this open.
        return Err(unsafe { RtlNtStatusToDosError(status) });
    }
    Ok(handle)
}

impl Drop for SecurityObject {
    fn drop(&mut self) {
        // SAFETY: each API returned an owned handle, closed here by the matching API.
        unsafe {
            if self.file {
                windows_sys::Win32::Foundation::CloseHandle(self.handle);
            } else {
                windows_sys::Win32::System::Registry::RegCloseKey(self.handle);
            }
        };
    }
}
