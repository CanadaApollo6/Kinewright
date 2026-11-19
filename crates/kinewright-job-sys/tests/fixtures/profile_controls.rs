use super::{Fixture, Local, SPAWNING, TIMEOUT, copy_wide, env_path, wide};
use kinewright_job_sys::media_lpac::{Error, Profile, StorageKind};
use std::{
    ffi::OsString,
    fs, io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, WAIT_OBJECT_0},
    Security::{
        ACL,
        Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
            ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT, SE_REGISTRY_KEY,
            SetNamedSecurityInfoW, SetSecurityInfo,
        },
        DACL_SECURITY_INFORMATION, GetTokenInformation, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, SECURITY_CAPABILITIES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    },
    Storage::FileSystem::READ_CONTROL,
    System::{
        Registry::{HKEY, HKEY_CURRENT_USER, REG_SAM_FLAGS, RegCloseKey, RegOpenKeyExW},
        Threading::{
            CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DETACHED_PROCESS,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
            GetExitCodeProcess, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
            OpenProcessToken, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION,
            STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

pub(super) fn current_user_sid() -> String {
    let mut raw = ptr::null_mut();
    // SAFETY: the current process pseudo handle is live, and raw is writable.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut raw) };
    assert_ne!(
        ok,
        0,
        "OpenProcessToken (control user): {}",
        io::Error::last_os_error()
    );
    // SAFETY: this guard receives the newly owned token.
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut user = [0usize; 32];
    let mut size = 0;
    // SAFETY: the fixed buffer is aligned and large enough for TOKEN_USER and a SID.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            user.as_mut_ptr().cast(),
            size_of_val(&user).try_into().expect("token buffer size"),
            &raw mut size,
        )
    };
    assert_ne!(
        ok,
        0,
        "GetTokenInformation (control TokenUser): {}",
        io::Error::last_os_error()
    );
    let mut text = ptr::null_mut();
    // SAFETY: the live token buffer contains a TOKEN_USER and the output is writable.
    let ok = unsafe {
        ConvertSidToStringSidW(
            (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid,
            &raw mut text,
        )
    };
    assert_ne!(
        ok,
        0,
        "ConvertSidToStringSidW (control user): {}",
        io::Error::last_os_error()
    );
    let _owned = Local(text.cast());
    // SAFETY: successful conversion supplied terminated UTF-16.
    unsafe { copy_wide(text) }
        .into_string()
        .expect("ASCII account SID")
}

pub(super) fn classes_key(profile: &Profile) -> String {
    let path = profile.registry_path();
    let key = path
        .strip_prefix(r"Software\Classes\")
        .expect("production key in the user classes hive");
    format!(r"{}_Classes\{key}", current_user_sid())
}

pub(super) struct Key(HKEY);

impl Key {
    pub(super) fn open(profile: &Profile, rights: REG_SAM_FLAGS) -> Result<Self, u32> {
        let path = wide(profile.registry_path());
        let mut key = ptr::null_mut();
        // SAFETY: path names the test profile key, and key is writable.
        let code =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, rights, &raw mut key) };
        if code == 0 { Ok(Self(key)) } else { Err(code) }
    }

    pub(super) fn owner(&self, user: &str, assign: bool) {
        witness_owner(self.0, SE_REGISTRY_KEY, user, assign);
    }

    pub(super) fn create_subkey(&self, name: &str) -> Self {
        use windows_sys::Win32::System::Registry::{KEY_ALL_ACCESS, RegCreateKeyExW};
        let name = wide(name);
        let mut key = ptr::null_mut();
        // SAFETY: the parent is an open test key and the terminated child name stays live.
        let code = unsafe {
            RegCreateKeyExW(
                self.0,
                name.as_ptr(),
                0,
                ptr::null(),
                0,
                KEY_ALL_ACCESS,
                ptr::null(),
                &raw mut key,
                ptr::null_mut(),
            )
        };
        assert_eq!(
            code, 0,
            "RegCreateKeyExW (test profile subkey) Win32 {code}"
        );
        Self(key)
    }

    pub(super) fn grant(&self, sid: &str) {
        let (descriptor, dacl) = descriptor(&format!(
            "D:P(A;;KA;;;SY)(A;;KA;;;BA)(A;;KA;;;{})(A;;KW;;;{sid})",
            current_user_sid()
        ));
        // SAFETY: both the key and descriptor stay live for the write to the test key.
        let code = unsafe {
            SetSecurityInfo(
                self.0,
                SE_REGISTRY_KEY,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null_mut(),
            )
        };
        drop(descriptor);
        assert_eq!(
            code, 0,
            "SetSecurityInfo (restore registry package ACE) Win32 {code}"
        );
    }

    pub(super) fn sddl(&self) -> String {
        let mut raw = ptr::null_mut();
        // SAFETY: the test key is open and raw receives its allocated descriptor.
        let code = unsafe {
            GetSecurityInfo(
                self.0,
                SE_REGISTRY_KEY,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &raw mut raw,
            )
        };
        assert_eq!(
            code, 0,
            "GetSecurityInfo (profile registry DACL) Win32 {code}"
        );
        let descriptor = Local(raw);
        sddl(&descriptor)
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: this owner closes its opened registry key.
        unsafe { RegCloseKey(self.0) };
    }
}

pub(super) fn file_owner(path: &Path, user: &str, assign: bool) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, WRITE_OWNER};
    let access = READ_CONTROL | if assign { WRITE_OWNER } else { 0 };
    let file = fs::OpenOptions::new()
        .access_mode(access)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .expect("CreateFileW (host owner setup)");
    witness_owner(file.as_raw_handle(), SE_FILE_OBJECT, user, assign);
}

pub(super) fn set_owner_control_dacl(path: &Path) {
    let (_descriptor, dacl) = descriptor("D:P(A;;FA;;;SY)");
    let path = wide(path);
    // SAFETY: the path names the fixture's scratch file and the DACL is live.
    let code = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        )
    };
    assert_eq!(
        code, 0,
        "SetNamedSecurityInfoW (implicit owner control) Win32 {code}"
    );
}

fn witness_owner(
    handle: windows_sys::Win32::Foundation::HANDLE,
    kind: windows_sys::Win32::Security::Authorization::SE_OBJECT_TYPE,
    user: &str,
    assign: bool,
) {
    use windows_sys::Win32::Security::{
        Authorization::ConvertStringSidToSidW, EqualSid, OWNER_SECURITY_INFORMATION,
    };
    let user = wide(user);
    let mut expected = ptr::null_mut();
    // SAFETY: the SID string is terminated and expected is writable.
    let ok = unsafe { ConvertStringSidToSidW(user.as_ptr(), &raw mut expected) };
    assert_ne!(
        ok,
        0,
        "ConvertStringSidToSidW (host owner): {}",
        io::Error::last_os_error()
    );
    let expected = Local(expected);
    if assign {
        // SAFETY: the checked fixture handle and expected owner SID are live.
        let code = unsafe {
            SetSecurityInfo(
                handle,
                kind,
                OWNER_SECURITY_INFORMATION,
                expected.0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        assert_eq!(code, 0, "SetSecurityInfo (host owner) Win32 {code}");
    }
    let (mut owner, mut descriptor) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: the fixture handle is live, and the selected output pointers are writable.
    let code = unsafe {
        GetSecurityInfo(
            handle,
            kind,
            OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    let _descriptor = Local(descriptor);
    assert_eq!(code, 0, "GetSecurityInfo (host owner) Win32 {code}");
    // SAFETY: both SIDs are backed by their live descriptors.
    assert!(
        !owner.is_null() && unsafe { EqualSid(owner, expected.0) } != 0,
        "readback owner differs from the current user SID"
    );
}

pub(super) fn registry_sddl(profile: &Profile) -> String {
    Key::open(profile, READ_CONTROL)
        .unwrap_or_else(|code| panic!("RegOpenKeyExW (profile snapshot) Win32 {code}"))
        .sddl()
}

fn descriptor(text: &str) -> (Local, *mut ACL) {
    use windows_sys::Win32::Security::GetSecurityDescriptorDacl;
    let text = wide(text);
    let mut raw = ptr::null_mut();
    // SAFETY: text is terminated and raw receives an owned descriptor.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            SDDL_REVISION_1,
            &raw mut raw,
            ptr::null_mut(),
        )
    };
    assert_ne!(
        ok,
        0,
        "ConvertStringSecurityDescriptorToSecurityDescriptorW: {}",
        io::Error::last_os_error()
    );
    let descriptor = Local(raw);
    let (mut present, mut defaulted, mut dacl) = (0, 0, ptr::null_mut());
    // SAFETY: the descriptor and all outputs are live.
    let ok = unsafe {
        GetSecurityDescriptorDacl(
            descriptor.0,
            &raw mut present,
            &raw mut dacl,
            &raw mut defaulted,
        )
    };
    assert!(
        ok != 0 && present != 0 && !dacl.is_null(),
        "GetSecurityDescriptorDacl (test ACE)"
    );
    (descriptor, dacl)
}

fn sddl(descriptor: &Local) -> String {
    try_sddl(descriptor, DACL_SECURITY_INFORMATION)
        .expect("ConvertSecurityDescriptorToStringSecurityDescriptorW (DACL snapshot)")
}

fn try_sddl(descriptor: &Local, information: u32) -> io::Result<String> {
    let mut text = ptr::null_mut();
    // SAFETY: descriptor is live and text receives an owned string.
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0,
            SDDL_REVISION_1,
            information,
            &raw mut text,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let _text = Local(text.cast());
    // SAFETY: the successful conversion supplied terminated UTF-16.
    unsafe { copy_wide(text) }
        .into_string()
        .map_err(|_| io::Error::other("invalid UTF-16 security descriptor"))
}

pub(super) fn report_prepare_failure(error: &Error) {
    let Some((kind, path)) = error.storage_object() else {
        eprintln!("prepare profile diagnostic: this failure did not identify a storage object");
        return;
    };
    let (name, object_type) = if kind == StorageKind::RegistryKey {
        let Ok(relative) = path.strip_prefix("HKEY_USERS") else {
            eprintln!(
                "prepare profile diagnostic: unexpected registry path {}",
                path.display()
            );
            return;
        };
        let mut name = OsString::from(r"USERS\");
        name.push(relative.as_os_str());
        (wide(name), SE_REGISTRY_KEY)
    } else {
        (wide(path), SE_FILE_OBJECT)
    };
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let (mut owner, mut raw) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: name is terminated and both selected outputs are writable.
    let code = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            object_type,
            information,
            &raw mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut raw,
        )
    };
    let descriptor = Local(raw);
    if code != 0 {
        eprintln!(
            "prepare profile diagnostic [{kind:?} {}]: GetNamedSecurityInfoW Win32 {code} ({code:#010x}), owner and SDDL unavailable",
            path.display()
        );
        return;
    }
    let mut text = ptr::null_mut();
    let owner = if owner.is_null() {
        "missing SID".to_owned()
    } else {
        // SAFETY: the owner SID is backed by the live descriptor and text is writable.
        if unsafe { ConvertSidToStringSidW(owner, &raw mut text) } == 0 {
            format!("ConvertSidToStringSidW: {}", io::Error::last_os_error())
        } else {
            let _text = Local(text.cast());
            // SAFETY: conversion returned allocated NUL-terminated UTF-16.
            unsafe { copy_wide(text) }.to_string_lossy().into_owned()
        }
    };
    let sddl = try_sddl(&descriptor, information).unwrap_or_else(|error| {
        format!("ConvertSecurityDescriptorToStringSecurityDescriptorW: {error}")
    });
    eprintln!(
        "prepare profile diagnostic [{kind:?} {}]: owner={owner}; SDDL={sddl}",
        path.display()
    );
}

pub(super) fn file_sddl(path: &Path) -> String {
    let path = wide(path);
    let mut raw = ptr::null_mut();
    // SAFETY: path names a test directory and raw receives the allocated descriptor.
    let code = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut raw,
        )
    };
    assert_eq!(
        code, 0,
        "GetNamedSecurityInfoW (test DACL snapshot) Win32 {code}"
    );
    let descriptor = Local(raw);
    sddl(&descriptor)
}

pub(super) fn grant_package_read(directory: &Path, file: &Path) {
    let user = current_user_sid();
    for (path, rights) in [(directory, "FRFX"), (file, "FR")] {
        let (descriptor, dacl) = descriptor(&format!("D:P(A;;FA;;;{user})(A;;{rights};;;AC)"));
        let path = wide(path);
        // SAFETY: path names a test canary and its DACL is backed by the live descriptor.
        let code = unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null_mut(),
            )
        };
        drop(descriptor);
        assert_eq!(
            code, 0,
            "SetNamedSecurityInfoW (ALL APPLICATION PACKAGES canary) Win32 {code}"
        );
    }
}

struct Attributes(LPPROC_THREAD_ATTRIBUTE_LIST);

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: this initialized list is dropped before its storage.
        unsafe { DeleteProcThreadAttributeList(self.0) };
    }
}

pub(super) fn plain_read(fixture: &Fixture, path: &Path) -> io::Result<u32> {
    let _spawning = SPAWNING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let local = fixture.root.0.join("ordinary environment");
    fs::create_dir(&local)?;
    let mut local_entry = OsString::from("LOCALAPPDATA=");
    local_entry.push(local);
    let mut root_entry = OsString::from("SystemRoot=");
    root_entry.push(env_path("SystemRoot"));
    let mut environment = wide(local_entry);
    environment.extend(wide(root_entry));
    environment.push(0);
    let app = wide(&fixture.exe);
    let cwd = wide(fixture.image_dir());
    // The test owns both paths and Windows filenames cannot contain a quote.
    let mut cmd = wide(format!(
        "\"{}\" --ordinary-package-control \"{}\"",
        fixture.exe.display(),
        path.display()
    ));
    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: fixture.sid.0,
        ..Default::default()
    };
    let mut bytes = 0;
    // SAFETY: this documented size query passes null storage and a writable size.
    unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &raw mut bytes) };
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER.cast_signed()) || bytes == 0 {
        return Err(error);
    }
    let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let list = storage.as_mut_ptr().cast();
    // SAFETY: storage is aligned and large enough for the initialized list.
    if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &raw mut bytes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _list = Attributes(list);
    // SAFETY: the capabilities structure and SID outlive the attribute list.
    if unsafe {
        UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            (&raw const capabilities).cast(),
            size_of_val(&capabilities),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>()
        .try_into()
        .expect("startup structure size");
    startup.lpAttributeList = list;
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: all inputs are live and terminated. This control inherits no handles.
    if unsafe {
        CreateProcessW(
            app.as_ptr(),
            cmd.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | DETACHED_PROCESS,
            environment.as_ptr().cast(),
            cwd.as_ptr(),
            &raw const startup.StartupInfo,
            &raw mut info,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    wait_control(info)
}

pub(super) fn inherited_canary(fixture: &Fixture) -> (OwnedHandle, String) {
    use windows_sys::Win32::{
        Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle},
        Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle},
    };
    let path = fixture.root.0.join("unlisted-file");
    fs::write(&path, b"unlisted handle").expect("WriteFile (unlisted canary)");
    let file = fs::File::open(path).expect("CreateFileW (unlisted canary)");
    let mut raw = ptr::null_mut();
    // SAFETY: file and pseudo process handle are live and raw is writable.
    let ok = unsafe {
        let owner = GetCurrentProcess();
        DuplicateHandle(
            owner,
            file.as_raw_handle(),
            owner,
            &raw mut raw,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    };
    assert_ne!(
        ok,
        0,
        "DuplicateHandle (inheritable canary): {}",
        io::Error::last_os_error()
    );
    // SAFETY: the successful call transferred ownership of the duplicate.
    let canary = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the exact canary is open and the output is writable.
    assert_ne!(
        unsafe { GetFileInformationByHandle(canary.as_raw_handle(), &raw mut info) },
        0,
        "GetFileInformationByHandle (unlisted canary): {}",
        io::Error::last_os_error()
    );
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    (
        canary,
        format!("{}:{}:{index}", raw as usize, info.dwVolumeSerialNumber),
    )
}

pub(super) fn ordinary_inheritance(fixture: &Fixture, target: &str) -> io::Result<u32> {
    // Use ordinary bInheritHandles without a STARTUPINFOEX handle list.
    let _spawning = SPAWNING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let app = wide(&fixture.exe);
    let mut command = wide(format!(
        r#""{}" --ordinary-handle-control {target}"#,
        fixture.exe.display()
    ));
    let cwd = wide(fixture.image_dir());
    let startup = windows_sys::Win32::System::Threading::STARTUPINFOW {
        cb: size_of::<windows_sys::Win32::System::Threading::STARTUPINFOW>()
            .try_into()
            .expect("startup size"),
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: all strings and structures are live. The explicit control inherits ordinary handles.
    if unsafe {
        CreateProcessW(
            app.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            DETACHED_PROCESS,
            ptr::null(),
            cwd.as_ptr(),
            &raw const startup,
            &raw mut info,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    wait_control(info)
}

pub(super) struct Junction<'a>(&'a Path);
impl Drop for Junction<'_> {
    fn drop(&mut self) {
        fs::remove_dir(self.0).expect("RemoveDirectoryW (test junction)");
    }
}

pub(super) fn junction<'a>(path: &'a Path, target: &Path) -> Junction<'a> {
    use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt};
    use windows_sys::Win32::{
        Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_WRITE,
        },
        System::IO::DeviceIoControl,
    };
    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00a4;
    fs::create_dir(path).expect("CreateDirectoryW (test junction)");
    let junction = Junction(path);
    let file = fs::OpenOptions::new()
        .access_mode(FILE_GENERIC_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .expect("CreateFileW (test junction)");
    let name: Vec<_> = target.as_os_str().encode_wide().collect();
    let mut substitute = vec![92u16, 63, 63, 92];
    substitute.extend(&name);
    let bytes = |units: usize| u16::try_from(units * 2).expect("junction name bytes");
    // Mount-point reparse buffer. Offsets and lengths are bytes relative to PathBuffer.
    let mut data = vec![
        3u16,
        0xa000,
        8 + bytes(substitute.len() + name.len() + 2),
        0,
        0,
        bytes(substitute.len()),
        bytes(substitute.len() + 1),
        bytes(name.len()),
    ];
    data.extend(substitute);
    data.push(0);
    data.extend(name);
    data.push(0);
    let mut returned = 0;
    // SAFETY: this sets a mount point on the freshly created test directory, with no output buffer.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_REPARSE_POINT,
            data.as_ptr().cast(),
            u32::try_from(size_of_val(data.as_slice())).expect("reparse buffer size"),
            ptr::null_mut(),
            0,
            &raw mut returned,
            ptr::null_mut(),
        )
    };
    assert_ne!(
        ok,
        0,
        "DeviceIoControl(FSCTL_SET_REPARSE_POINT): {}",
        io::Error::last_os_error()
    );
    junction
}

fn wait_control(info: PROCESS_INFORMATION) -> io::Result<u32> {
    // SAFETY: CreateProcessW transferred ownership of both handles.
    let (process, _thread) = unsafe {
        (
            OwnedHandle::from_raw_handle(info.hProcess),
            OwnedHandle::from_raw_handle(info.hThread),
        )
    };
    let timeout = u32::try_from(TIMEOUT.as_millis()).expect("fixture timeout");
    // SAFETY: the live process handle can be waited on.
    let result = unsafe { WaitForSingleObject(process.as_raw_handle(), timeout) };
    if result != WAIT_OBJECT_0 {
        // SAFETY: this is the process created above and the handle grants termination.
        unsafe { TerminateProcess(process.as_raw_handle(), 1) };
        return Err(io::Error::other(format!(
            "WaitForSingleObject (ordinary AppContainer): {result}"
        )));
    }
    let mut exit = 0;
    // SAFETY: process is live and exit is writable.
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &raw mut exit) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(exit)
}
