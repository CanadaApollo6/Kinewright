use super::super::{Local, Sid, copy_wide, wide};

#[path = "../fixtures/lpac_registry_key.rs"]
mod registry_key;

use registry_key::open_registry;
use std::{
    ffi::OsStr,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_INVALID_ACL, ERROR_PATH_NOT_FOUND, HANDLE},
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSidToSidW,
            EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, GetSecurityInfo,
            SDDL_REVISION_1, SE_FILE_OBJECT, SE_REGISTRY_KEY, SetEntriesInAclW,
            SetNamedSecurityInfoW, SetSecurityInfo, TRUSTEE_IS_SID,
        },
        DACL_SECURITY_INFORMATION, DeleteAce, EqualSid, GROUP_SECURITY_INFORMATION, GetAce,
        GetSecurityDescriptorControl, GetTokenInformation, IsValidAcl, NO_INHERITANCE,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED,
        TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, READ_CONTROL, WRITE_DAC,
    },
    System::{
        Registry::{KEY_READ, KEY_SET_VALUE, REG_BINARY, RegSetValueExW},
        SystemServices::{
            ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_CALLBACK_ACE_TYPE, ACCESS_DENIED_ACE_TYPE,
            ACCESS_DENIED_CALLBACK_ACE_TYPE,
        },
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const OWNERSHIP: u32 = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION;
const SECURITY: u32 = OWNERSHIP | DACL_SECURITY_INFORMATION;
pub(super) fn registry_state(name: &str) -> String {
    match open_registry(name, KEY_READ) {
        Ok(_) => "NtOpenKey Win32 0 (still present)".into(),
        Err(error) => {
            let state = if matches!(error.code, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) {
                "absent"
            } else {
                "unverified"
            };
            format!("{error} ({state})")
        }
    }
}

pub(super) fn seed_registry(name: &str, value: &str) -> Result<(), String> {
    let key = open_registry(name, KEY_SET_VALUE)
        .map_err(|error| format!("seed profile value: {error}"))?;
    let value = wide(OsStr::new(value));
    // SAFETY: key is live, value is terminated and the bytes are a fixed eight-byte canary.
    let status = unsafe {
        RegSetValueExW(
            key.0,
            value.as_ptr(),
            0,
            REG_BINARY,
            b"baseline".as_ptr(),
            8,
        )
    };
    status_result("RegSetValueExW (seed profile value)", status)
}

enum Object<'a> {
    File(&'a [u16]),
    Registry(HANDLE),
}

impl Object<'_> {
    fn read(&self) -> Result<(Local, *mut ACL), String> {
        let mut dacl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        // SAFETY: the target is the test profile object and the outputs are writable.
        let code = unsafe {
            match self {
                Self::File(name) => GetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    SECURITY,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &raw mut dacl,
                    ptr::null_mut(),
                    &raw mut descriptor,
                ),
                Self::Registry(key) => GetSecurityInfo(
                    *key,
                    SE_REGISTRY_KEY,
                    SECURITY,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &raw mut dacl,
                    ptr::null_mut(),
                    &raw mut descriptor,
                ),
            }
        };
        let descriptor = Local(descriptor);
        let api = match self {
            Self::File(_) => "GetNamedSecurityInfoW (DACL)",
            Self::Registry(_) => "GetSecurityInfo (DACL)",
        };
        status_result(api, code)?;
        Ok((descriptor, dacl))
    }

    fn set(&self, dacl: *mut ACL, protection: u32) -> Result<(), String> {
        // SAFETY: the target is the owned profile object and the new DACL stays live.
        let code = unsafe {
            match self {
                Self::File(name) => SetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | protection,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    dacl,
                    ptr::null_mut(),
                ),
                Self::Registry(key) => SetSecurityInfo(
                    *key,
                    SE_REGISTRY_KEY,
                    DACL_SECURITY_INFORMATION | protection,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    dacl,
                    ptr::null_mut(),
                ),
            }
        };
        let api = match self {
            Self::File(_) => "SetNamedSecurityInfoW (DACL)",
            Self::Registry(_) => "SetSecurityInfo (DACL)",
        };
        status_result(api, code)
    }
}

fn sddl(descriptor: &Local) -> Result<String, String> {
    let mut text = ptr::null_mut();
    // SAFETY: the owned descriptor and writable string pointer are live until return.
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0,
            SDDL_REVISION_1,
            SECURITY,
            &raw mut text,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(format!(
            "ConvertSecurityDescriptorToStringSecurityDescriptorW: {}",
            std::io::Error::last_os_error()
        ));
    }
    let text = Local(text.cast());
    // SAFETY: this successful conversion returned an owned terminated string.
    Ok(unsafe { copy_wide(text.0.cast()) }
        .to_string_lossy()
        .into_owned())
}

fn convert_sid(text: &str) -> Result<Local, String> {
    let text = wide(OsStr::new(text));
    let mut sid = ptr::null_mut();
    // SAFETY: text is terminated and sid is a writable out pointer.
    if unsafe { ConvertStringSidToSidW(text.as_ptr(), &raw mut sid) } == 0 {
        return Err(format!(
            "ConvertStringSidToSidW: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(Local(sid))
}

fn entry(sid: PSID, mode: i32, rights: u32, inheritance: u32) -> EXPLICIT_ACCESS_W {
    let mut entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: rights,
        grfAccessMode: mode,
        grfInheritance: inheritance,
        ..Default::default()
    };
    entry.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    entry.Trustee.ptstrName = sid.cast();
    entry
}

pub(super) fn grant_package_read(directory: &Path, file: &Path) -> Result<(), String> {
    let mut token = ptr::null_mut();
    // SAFETY: GetCurrentProcess provides the live host pseudo-handle, token is writable.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(format!(
            "OpenProcessToken (package-only host user): {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: a successful OpenProcessToken hands ownership of token to this scope.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut user = [0usize; 32];
    let mut size = 0;
    // SAFETY: the live token and the aligned, fixed-capacity user SID buffer are valid.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            user.as_mut_ptr().cast(),
            size_of_val(&user)
                .try_into()
                .expect("host token user buffer size"),
            &raw mut size,
        )
    } == 0
    {
        return Err(format!(
            "GetTokenInformation (package-only host TokenUser): {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: the successful call populated TOKEN_USER and its SID in this buffer.
    let user_sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let all_packages = convert_sid("S-1-15-2-1")?;
    for (path, access) in [
        (directory, FILE_GENERIC_READ | FILE_GENERIC_EXECUTE),
        (file, FILE_GENERIC_READ),
    ] {
        let name = wide(path.as_os_str());
        let object = Object::File(&name);
        let grants = [
            entry(user_sid, GRANT_ACCESS, FILE_ALL_ACCESS, NO_INHERITANCE),
            entry(all_packages.0, GRANT_ACCESS, access, NO_INHERITANCE),
        ];
        let mut updated = ptr::null_mut();
        // SAFETY: both SIDs are live and updated is writable. Start with an empty ACL.
        let code = unsafe { SetEntriesInAclW(2, grants.as_ptr(), ptr::null(), &raw mut updated) };
        let _updated = Local(updated.cast());
        status_result("SetEntriesInAclW (package-only canary)", code)?;
        if updated.is_null() {
            return Err(
                "SetEntriesInAclW (package-only canary) Win32 0 returned a null DACL".into(),
            );
        }
        object.set(updated, PROTECTED_DACL_SECURITY_INFORMATION)?;
        let (descriptor, _) = object.read()?;
        let after = sddl(&descriptor)?;
        eprintln!(
            "SetNamedSecurityInfoW (package-only canary) Win32 0 [{}] {after}",
            path.display()
        );
    }
    Ok(())
}

fn package_entries(dacl: *mut ACL, sid: &Sid) -> Result<Vec<(u32, String)>, String> {
    if dacl.is_null() {
        return Err(format!(
            "GetSecurityInfo returned a null DACL (Win32 {ERROR_INVALID_ACL})"
        ));
    }
    // SAFETY: dacl is borrowed from a live descriptor returned by Windows.
    if unsafe { IsValidAcl(dacl) } == 0 {
        return Err(format!("IsValidAcl Win32 {ERROR_INVALID_ACL}"));
    }
    let all = convert_sid("S-1-15-2-1")?;
    let restricted = convert_sid("S-1-15-2-2")?;
    let package = sid.text();
    let targets = [
        (sid.0, package.as_os_str()),
        (all.0, OsStr::new("S-1-15-2-1")),
        (restricted.0, OsStr::new("S-1-15-2-2")),
    ];
    // SAFETY: IsValidAcl validated the ACL, including its header.
    let count = unsafe { (*dacl).AceCount };
    let mut matches = Vec::new();
    for index in 0..u32::from(count) {
        let mut ace = ptr::null_mut();
        // SAFETY: index is in the validated ACL, and ace receives a borrowed pointer.
        if unsafe { GetAce(dacl, index, &raw mut ace) } == 0 {
            return Err(format!(
                "GetAce entry {index}: {}",
                io::Error::last_os_error()
            ));
        }
        // SAFETY: GetAce returned the header of a live, validated ACE.
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        let kind = u32::from(header.AceType);
        let entry = format!(
            "entry {index} type={kind:#04x} flags={:#04x}",
            header.AceFlags
        );
        let offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        if !matches!(
            kind,
            ACCESS_ALLOWED_ACE_TYPE
                | ACCESS_DENIED_ACE_TYPE
                | ACCESS_ALLOWED_CALLBACK_ACE_TYPE
                | ACCESS_DENIED_CALLBACK_ACE_TYPE
        ) || usize::from(header.AceSize) < offset + 8
        {
            return Err(format!(
                "GetAce Win32 0, cannot identify trustee of {entry} (Win32 {ERROR_INVALID_ACL})"
            ));
        }
        // SAFETY: the supported ACE types share this SID offset, and the fixed SID header fits.
        let trustee = unsafe { ace.cast::<u8>().add(offset) };
        // SAFETY: byte 1 is the SID subauthority count at the verified offset.
        let subauthorities = usize::from(unsafe { *trustee.add(1) });
        if offset + 8 + 4 * subauthorities > usize::from(header.AceSize) {
            return Err(format!(
                "GetAce Win32 0, invalid trustee SID in {entry} (Win32 {ERROR_INVALID_ACL})"
            ));
        }
        for (expected, name) in targets {
            // SAFETY: both SIDs are live, with their subauthorities inside their buffers.
            if unsafe { EqualSid(trustee.cast(), expected) } != 0 {
                matches.push((index, format!("{entry} SID={}", name.display())));
            }
        }
    }
    Ok(matches)
}

fn verify_object(object: &Object<'_>, label: &str, stage: &str, sid: &Sid) -> Result<(), String> {
    let (descriptor, dacl) = object.read()?;
    eprintln!("{stage} SDDL [{label}] {}", sddl(&descriptor)?);
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: descriptor is live and the control flags and revision are writable.
    if unsafe { GetSecurityDescriptorControl(descriptor.0, &raw mut control, &raw mut revision) }
        == 0
    {
        return Err(format!(
            "GetSecurityDescriptorControl: {}",
            io::Error::last_os_error()
        ));
    }
    let remaining = package_entries(dacl, sid)?;
    if !remaining.is_empty() || control & SE_DACL_PROTECTED == 0 {
        return Err(format!(
            "GetSecurityInfo Win32 0 [{label}] protected={} package entries={remaining:?}",
            control & SE_DACL_PROTECTED != 0
        ));
    }
    eprintln!("{stage} ACL readback [{label}] protected=true package entries=0");
    Ok(())
}

fn harden(object: &Object<'_>, label: &str, sid: &Sid) -> Result<(), String> {
    let (descriptor, dacl) = object.read()?;
    eprintln!("j/l SDDL before [{label}] {}", sddl(&descriptor)?);
    let entries = package_entries(dacl, sid)?;
    // Delete from the last slot so earlier slot numbers stay valid.
    for (index, entry) in entries.iter().rev() {
        // SAFETY: dacl is writable memory from GetSecurityInfo and index names a live ACE.
        if unsafe { DeleteAce(dacl, *index) } == 0 {
            return Err(format!(
                "DeleteAce [{label}] {entry}: {}",
                io::Error::last_os_error()
            ));
        }
        eprintln!("DeleteAce Win32 0 [{label}] {entry}");
    }
    let result = object
        .set(dacl, PROTECTED_DACL_SECURITY_INFORMATION)
        .map_err(|error| format!("{error} [{label}] removed entries={entries:?}"));
    let observed = verify_object(object, label, "j/l after", sid);
    result.and(observed)
}

pub(super) fn verify_file(path: &Path, stage: &str, sid: &Sid) -> Result<(), String> {
    let name = wide(path.as_os_str());
    verify_object(
        &Object::File(&name),
        &path.display().to_string(),
        stage,
        sid,
    )
}

pub(super) fn file(path: &Path, sid: &Sid) -> Result<(), String> {
    let name = wide(path.as_os_str());
    harden(&Object::File(&name), &path.display().to_string(), sid)
}

pub(super) fn registry(name: &str, sid: &Sid) -> Result<(), String> {
    let key = open_registry(name, READ_CONTROL | WRITE_DAC)
        .map_err(|error| format!("open profile registry for hardening: {error}"))?;
    harden(&Object::Registry(key.0), name, sid)
}

pub(super) fn verify_registry(name: &str, stage: &str, sid: &Sid) -> Result<(), String> {
    let key = open_registry(name, READ_CONTROL)
        .map_err(|error| format!("open profile registry for readback: {error}"))?;
    verify_object(&Object::Registry(key.0), name, stage, sid)
}

fn status_result(api: &str, code: u32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{api} Win32 {code}"))
    }
}
