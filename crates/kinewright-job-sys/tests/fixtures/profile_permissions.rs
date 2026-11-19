use super::profile_registry;
use kinewright_job_sys::media_lpac::diagnostic::CANDIDATE_ENVIRONMENT;
use std::{
    env,
    ffi::OsStr,
    io::{self, Write},
    mem::size_of_val,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_INVALID_DATA, GetLastError, HANDLE, LocalFree},
    Security::{
        ACL,
        Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT, SE_REGISTRY_KEY,
            SetNamedSecurityInfoW, SetSecurityInfo,
        },
        DACL_SECURITY_INFORMATION, EqualSid, GetSecurityDescriptorDacl, GetTokenInformation,
        Isolation::GetAppContainerNamedObjectPath,
        OWNER_SECURITY_INFORMATION, PSID, TOKEN_GROUPS, TOKEN_QUERY, TOKEN_USER, TokenGroups,
        TokenUser,
    },
    Storage::FileSystem::{READ_CONTROL, WRITE_DAC},
    System::{
        SystemServices::{SE_GROUP_ENABLED, SE_GROUP_OWNER},
        Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken, SetEvent},
    },
};

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

fn last_error() -> u32 {
    // SAFETY: read this thread's last error after a failed Win32 call.
    unsafe { GetLastError() }
}

pub(super) fn environment(output: &mut impl Write) -> io::Result<()> {
    let names = env::vars_os().map(|(name, _)| name).collect::<Vec<_>>();
    let paths = ["SystemRoot"]
        .into_iter()
        .chain(CANDIDATE_ENVIRONMENT)
        .map(|key| (key, env::var_os(key)))
        .collect::<Vec<_>>();
    writeln!(
        output,
        "GetEnvironmentStringsW\t0\tnames={names:?} Windows paths={paths:?}"
    )
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

struct Local(*mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: this owner holds a LocalAlloc descriptor, or null.
        unsafe { LocalFree(self.0) };
    }
}

fn dacl(sddl: &OsStr) -> Result<(Local, *mut ACL), u32> {
    let sddl = wide(sddl);
    let mut descriptor = ptr::null_mut();
    // SAFETY: the input is terminated and descriptor is writable.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &raw mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(last_error());
    }
    let descriptor = Local(descriptor);
    let (mut present, mut defaulted, mut acl) = (0, 0, ptr::null_mut());
    // SAFETY: the owned descriptor remains live and all outputs are writable.
    if unsafe {
        GetSecurityDescriptorDacl(
            descriptor.0,
            &raw mut present,
            &raw mut acl,
            &raw mut defaulted,
        )
    } == 0
    {
        return Err(last_error());
    }
    if present == 0 || acl.is_null() {
        return Err(ERROR_INVALID_DATA);
    }
    Ok((descriptor, acl))
}

fn ownership(owner: PSID) -> String {
    if owner.is_null() {
        return "owner unknown".into();
    }
    match token().and_then(|token| token_owns(&token, owner)) {
        Ok(true) => "owner SID is in token".into(),
        Ok(false) => "non-owner token".into(),
        Err(code) => format!("ownership query Win32 {code}"),
    }
}

fn token_owns(token: &OwnedHandle, owner: PSID) -> Result<bool, u32> {
    let mut data = [0usize; 512];
    let mut returned = 0;
    // SAFETY: token is live and the output is aligned and writable.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            data.as_mut_ptr().cast(),
            u32::try_from(size_of_val(&data)).expect("bounded token buffer"),
            &raw mut returned,
        )
    } == 0
    {
        return Err(last_error());
    }
    // SAFETY: the successful query populated a TOKEN_USER and its SID.
    if unsafe { EqualSid((*data.as_ptr().cast::<TOKEN_USER>()).User.Sid, owner) } != 0 {
        return Ok(true);
    }
    // SAFETY: same live token and aligned output as above, now for the group array.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenGroups,
            data.as_mut_ptr().cast(),
            u32::try_from(size_of_val(&data)).expect("bounded token buffer"),
            &raw mut returned,
        )
    } == 0
    {
        return Err(last_error());
    }
    // SAFETY: the successful query populated the header and the group array it describes.
    Ok(unsafe {
        let groups = data.as_ptr().cast::<TOKEN_GROUPS>();
        let count = (*groups).GroupCount as usize;
        let first = (&raw const (*groups).Groups)
            .cast::<windows_sys::Win32::Security::SID_AND_ATTRIBUTES>();
        (0..count).any(|idx| {
            let group = &*first.add(idx);
            let flags = (SE_GROUP_OWNER | SE_GROUP_ENABLED).cast_unsigned();
            group.Attributes & flags == flags && EqualSid(group.Sid, owner) != 0
        })
    })
}

pub(super) fn restore_file(target: &OsStr, output: &mut impl Write) -> io::Result<()> {
    let text = target.to_string_lossy();
    let Some((path, sddl)) = text.split_once('\t') else {
        return writeln!(output, "fixture restore arguments\t{ERROR_INVALID_DATA}");
    };
    let (_descriptor, acl) = match dacl(OsStr::new(sddl)) {
        Ok(result) => result,
        Err(code) => return writeln!(output, "Convert/GetSecurityDescriptorDacl\t{code}"),
    };
    let path = wide(OsStr::new(path));
    let (mut owner, mut descriptor) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: the name is terminated and both outputs are writable.
    let owner_code = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    let _owner_descriptor = Local(descriptor);
    let identity = if owner_code == 0 {
        ownership(owner)
    } else {
        format!("GetNamedSecurityInfoW(owner) Win32 {owner_code}")
    };
    // SAFETY: path identifies this test's owned profile object and acl is retained by descriptor.
    let status = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null_mut(),
        )
    };
    writeln!(
        output,
        "SetNamedSecurityInfoW(restore profile DACL)\t{status}\t{identity}"
    )
}

pub(super) fn restore_registry(sddl: &OsStr, output: &mut impl Write) -> io::Result<()> {
    let (_descriptor, acl) = match dacl(sddl) {
        Ok(result) => result,
        Err(code) => return writeln!(output, "Convert/GetSecurityDescriptorDacl\t{code}"),
    };
    let identity = match profile_registry::open(READ_CONTROL) {
        Ok(key) => registry_owner(key.0),
        Err(status) => format!(
            "GetAppContainerRegistryLocation(READ_CONTROL) HRESULT {:#010x}",
            status.cast_unsigned()
        ),
    };
    let key = match profile_registry::open(WRITE_DAC) {
        Ok(key) => key,
        Err(status) => {
            return writeln!(
                output,
                "GetAppContainerRegistryLocation(WRITE_DAC) HRESULT\t{}\t{identity}",
                status.cast_unsigned()
            );
        }
    };
    // SAFETY: this is the current owned profile root and acl is retained by descriptor.
    let status = unsafe {
        SetSecurityInfo(
            key.0,
            SE_REGISTRY_KEY,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null_mut(),
        )
    };
    writeln!(
        output,
        "SetSecurityInfo(restore registry DACL)\t{status}\t{identity}"
    )
}

fn registry_owner(root: HANDLE) -> String {
    let (mut owner, mut descriptor) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: root is live and both outputs are writable.
    let status = unsafe {
        GetSecurityInfo(
            root,
            SE_REGISTRY_KEY,
            OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    let _descriptor = Local(descriptor);
    if status == 0 {
        ownership(owner)
    } else {
        format!("GetSecurityInfo(owner) Win32 {status}")
    }
}
