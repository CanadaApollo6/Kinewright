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
        "file" => true,
        "key" => false,
        _ => return Err(io::Error::other("unknown owner access object")),
    };
    let mask = if file { "FA" } else { "KA" };
    let sddl = format!(
        "O:{owner}D:P(A;;{mask};;;SY)(A;;{mask};;;BA)(A;;{mask};;;{owner})(A;;{mask};;;{package})"
    );
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
    let api = if file { "CreateFileW" } else { "RegOpenKeyExW" };
    let handle = match SecurityObject::open(path, file, access) {
        Ok(handle) => handle,
        Err(code) => {
            return writeln!(
                output,
                "{api}({right})\t{code}\tTokenUser matches owner SID={same}"
            );
        }
    };
    let mut detail = format!("TokenUser matches owner SID={same}");
    if access == WRITE_OWNER {
        let owner_code = handle.set_owner(&descriptor);
        let dacl_code = handle.set_dacl(&descriptor);
        let followup = match SecurityObject::open(path, file, WRITE_DAC) {
            Ok(writable) => format!(
                "open WRITE_DAC=0 SetSecurityInfo(DACL)={}",
                writable.set_dacl(&descriptor)
            ),
            Err(code) => format!("open WRITE_DAC={code}"),
        };
        write!(
            detail,
            " SetSecurityInfo(OWNER)={owner_code} SetSecurityInfo(DACL via WRITE_OWNER)={dacl_code} {followup}"
        )
        .expect("string formatting");
    } else {
        write!(
            detail,
            " SetSecurityInfo(DACL)={}",
            handle.set_dacl(&descriptor)
        )
        .expect("string formatting");
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
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            Storage::FileSystem::{
                CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
                FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            },
            System::Registry::{HKEY_USERS, RegOpenKeyExW},
        };
        let path = wide(OsStr::new(path));
        let handle = if file {
            // SAFETY: the path is a terminated name for a host-owned witness, without reparse traversal.
            let handle = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    access,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                    ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(last_error());
            }
            handle
        } else {
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

    fn object_type(&self) -> windows_sys::Win32::Security::Authorization::SE_OBJECT_TYPE {
        use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SE_REGISTRY_KEY};
        if self.file {
            SE_FILE_OBJECT
        } else {
            SE_REGISTRY_KEY
        }
    }

    fn set_dacl(&self, descriptor: &OwnerDescriptor) -> u32 {
        use windows_sys::Win32::Security::{
            Authorization::SetSecurityInfo, DACL_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION,
        };
        // SAFETY: this is the fixture handle and its explicit canary DACL stays live for the write.
        unsafe {
            SetSecurityInfo(
                self.handle,
                self.object_type(),
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
            Authorization::SetSecurityInfo, OWNER_SECURITY_INFORMATION,
        };
        // SAFETY: this is the fixture handle and the owner SID remains live.
        unsafe {
            SetSecurityInfo(
                self.handle,
                self.object_type(),
                OWNER_SECURITY_INFORMATION,
                descriptor.owner,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        }
    }
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
