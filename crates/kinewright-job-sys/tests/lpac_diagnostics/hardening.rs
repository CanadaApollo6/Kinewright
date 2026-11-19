use super::super::{Local, Sid, copy_wide, wide};
use std::{ffi::OsStr, mem::size_of, path::Path, ptr};
use windows_sys::{
    Wdk::{Foundation::OBJECT_ATTRIBUTES, System::Registry::NtOpenKey},
    Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, HANDLE, OBJ_CASE_INSENSITIVE,
            RtlNtStatusToDosError, UNICODE_STRING,
        },
        Security::{
            ACL,
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSidToSidW,
                DENY_ACCESS, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW,
                GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT, SE_REGISTRY_KEY,
                SetEntriesInAclW, SetNamedSecurityInfoW, SetSecurityInfo, TRUSTEE_IS_SID,
            },
            CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION,
            NO_INHERITANCE, OWNER_SECURITY_INFORMATION, PSID, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        },
        Storage::FileSystem::{
            DELETE, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA,
            FILE_WRITE_EA, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
        },
        System::Registry::{
            HKEY, KEY_CREATE_LINK, KEY_CREATE_SUB_KEY, KEY_READ, KEY_SET_VALUE, REG_BINARY,
            RegCloseKey, RegSetValueExW,
        },
    },
};

const OWNERSHIP: u32 = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION;
const SECURITY: u32 = OWNERSHIP | DACL_SECURITY_INFORMATION;
const CHANGE_SECURITY: u32 = DELETE | WRITE_DAC | WRITE_OWNER;
const FILE_MUTATION: u32 = CHANGE_SECURITY
    | FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_DELETE_CHILD
    | FILE_WRITE_ATTRIBUTES
    | FILE_WRITE_EA;
const KEY_MUTATION: u32 = CHANGE_SECURITY | KEY_SET_VALUE | KEY_CREATE_SUB_KEY | KEY_CREATE_LINK;

pub(super) struct RegistryKey(HKEY);
impl Drop for RegistryKey {
    fn drop(&mut self) {
        // SAFETY: NtOpenKey transferred this registry handle to this owner.
        unsafe { RegCloseKey(self.0) };
    }
}

struct RegistryError {
    api: &'static str,
    status: Option<i32>,
    code: u32,
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} Win32 {}", self.api, self.code)?;
        if let Some(status) = self.status {
            write!(f, ", NTSTATUS {:#010x}", status.cast_unsigned())?;
        }
        Ok(())
    }
}

fn open_registry(name: &str, rights: u32) -> Result<RegistryKey, RegistryError> {
    let mut units = wide(OsStr::new(name));
    let length = u16::try_from((units.len() - 1) * 2).map_err(|_| RegistryError {
        api: "NtOpenKey input length",
        status: None,
        code: windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER,
    })?;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: units.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>()
            .try_into()
            .expect("OBJECT_ATTRIBUTES size"),
        ObjectName: &raw const name,
        Attributes: OBJ_CASE_INSENSITIVE,
        ..Default::default()
    };
    let mut handle = ptr::null_mut();
    // SAFETY: the owned profile key name and attrs are live and handle is writable.
    let status = unsafe { NtOpenKey(&raw mut handle, rights, &raw const attributes) };
    if status >= 0 {
        Ok(RegistryKey(handle))
    } else {
        // SAFETY: the conversion takes an NTSTATUS value.
        let code = unsafe { RtlNtStatusToDosError(status) };
        Err(RegistryError {
            api: "NtOpenKey",
            status: Some(status),
            code,
        })
    }
}

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
        status_result("GetNamedSecurityInfoW/GetSecurityInfo (profile)", code)?;
        Ok((descriptor, dacl))
    }

    fn set(&self, dacl: *mut ACL) -> Result<(), String> {
        // SAFETY: the target is the owned profile object and the new DACL stays live.
        let code = unsafe {
            match self {
                Self::File(name) => SetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    dacl,
                    ptr::null_mut(),
                ),
                Self::Registry(key) => SetSecurityInfo(
                    *key,
                    SE_REGISTRY_KEY,
                    DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    dacl,
                    ptr::null_mut(),
                ),
            }
        };
        status_result("SetNamedSecurityInfoW/SetSecurityInfo (profile DACL)", code)
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

fn harden(
    object: &Object<'_>,
    label: &str,
    sid: &Sid,
    mask: u32,
    inheritance: u32,
) -> Result<String, String> {
    let (descriptor, old_dacl) = object.read()?;
    let before = sddl(&descriptor)?;
    eprintln!("j SDDL before [{label}] {before}");
    if old_dacl.is_null() {
        return Err("GetSecurityInfo (profile) returned a null DACL".into());
    }
    let all_packages = convert_sid("S-1-15-2-1")?;
    let restricted_packages = convert_sid("S-1-15-2-2")?;
    let owner_rights = convert_sid("S-1-3-4")?;
    let entries = [
        entry(sid.0, DENY_ACCESS, mask, inheritance),
        entry(all_packages.0, DENY_ACCESS, mask, inheritance),
        entry(restricted_packages.0, DENY_ACCESS, mask, inheritance),
        // An OWNER RIGHTS ACE suppresses the owner's implicit WRITE_DAC grant.
        entry(owner_rights.0, GRANT_ACCESS, READ_CONTROL, inheritance),
    ];
    let mut new_dacl = ptr::null_mut();
    // SAFETY: the existing descriptor, array and all SIDs remain live through the merge.
    let code = unsafe {
        SetEntriesInAclW(
            entries.len().try_into().expect("ACL entries"),
            entries.as_ptr(),
            old_dacl,
            &raw mut new_dacl,
        )
    };
    let _allocation = Local(new_dacl.cast());
    status_result("SetEntriesInAclW (profile mutation denies)", code)?;
    if new_dacl.is_null() {
        return Err("SetEntriesInAclW Win32 0 returned a null DACL".into());
    }
    let result = object.set(new_dacl);
    let after = object.read().and_then(|(descriptor, _)| sddl(&descriptor));
    match &after {
        Ok(after) => eprintln!("j SDDL after [{label}] {after}"),
        Err(error) => eprintln!("j SDDL after [{label}] unavailable: {error}"),
    }
    result?;
    after?;
    Ok(before)
}

pub(super) fn final_file_sddl(path: &Path) -> Result<String, String> {
    let name = wide(path.as_os_str());
    let (descriptor, _) = Object::File(&name).read()?;
    sddl(&descriptor)
}

pub(super) fn file(path: &Path, is_dir: bool, sid: &Sid) -> Result<String, String> {
    let name = wide(path.as_os_str());
    harden(
        &Object::File(&name),
        &path.display().to_string(),
        sid,
        FILE_MUTATION,
        if is_dir {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            NO_INHERITANCE
        },
    )
}

pub(super) fn registry(name: &str, sid: &Sid) -> Result<String, String> {
    let key = open_registry(name, READ_CONTROL | WRITE_DAC)
        .map_err(|error| format!("open profile registry for hardening: {error}"))?;
    harden(
        &Object::Registry(key.0),
        name,
        sid,
        KEY_MUTATION,
        CONTAINER_INHERIT_ACE,
    )
}

fn status_result(api: &str, code: u32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{api} Win32 {code}"))
    }
}
