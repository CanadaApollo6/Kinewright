use super::{Code, Error, Sid, Step, StorageKind, sid_text, wide};
use std::{
    ffi::{OsStr, c_void},
    fs::File,
    mem::offset_of,
    os::windows::io::AsRawHandle,
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_INVALID_ACL, HANDLE, LocalFree},
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        Authorization::{
            ConvertStringSidToSidW, GetSecurityInfo, SE_FILE_OBJECT, SE_OBJECT_TYPE,
            SE_REGISTRY_KEY, SetSecurityInfo,
        },
        DACL_SECURITY_INFORMATION, DeleteAce, EqualSid, GetAce, GetSecurityDescriptorControl,
        IsValidAcl, PROTECTED_DACL_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED,
    },
    System::SystemServices::{
        ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_CALLBACK_ACE_TYPE, ACCESS_DENIED_ACE_TYPE,
        ACCESS_DENIED_CALLBACK_ACE_TYPE,
    },
};

struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: these security APIs transfer LocalAlloc memory to this guard.
        unsafe { LocalFree(self.0) };
    }
}

pub(super) struct PackageSids<'a> {
    package: &'a Sid,
    all: Local,
    restricted: Local,
}

impl<'a> PackageSids<'a> {
    pub(super) fn new(package: &'a Sid) -> Result<Self, Error> {
        Ok(Self {
            package,
            all: parse_sid("S-1-15-2-1")?,
            restricted: parse_sid("S-1-15-2-2")?,
        })
    }
}

fn parse_sid(text: &str) -> Result<Local, Error> {
    let name = wide(OsStr::new(text))
        .map_err(|error| Error::launch(error, Step::ProfileIdentity, "ConvertStringSidToSidW"))?;
    let mut sid = ptr::null_mut();
    // SAFETY: name is terminated and sid is writable.
    if unsafe { ConvertStringSidToSidW(name.as_ptr(), &raw mut sid) } == 0 {
        return Err(Error::last(
            Step::ProfileIdentity,
            "ConvertStringSidToSidW",
            text,
        ));
    }
    Ok(Local(sid))
}

pub(super) struct Object<'a> {
    handle: HANDLE,
    security_type: SE_OBJECT_TYPE,
    kind: StorageKind,
    path: &'a Path,
    access: u32,
}

impl<'a> Object<'a> {
    pub(super) fn file(file: &'a File, path: &'a Path, kind: StorageKind, access: u32) -> Self {
        Self {
            handle: file.as_raw_handle(),
            security_type: SE_FILE_OBJECT,
            kind,
            path,
            access,
        }
    }

    pub(super) fn key(handle: HANDLE, path: &'a Path, access: u32) -> Self {
        Self {
            handle,
            security_type: SE_REGISTRY_KEY,
            kind: StorageKind::RegistryKey,
            path,
            access,
        }
    }

    fn at_object(&self, error: Error) -> Error {
        error.at_storage(self.kind, self.path, self.access)
    }

    fn error(&self, api: &str, code: Code, detail: impl Into<String>) -> Error {
        self.at_object(Error::new(Step::StorageAcl, api, code, detail))
    }

    fn read(&self) -> Result<(Local, *mut ACL), Error> {
        let (mut descriptor, mut dacl) = (ptr::null_mut(), ptr::null_mut());
        // SAFETY: handle names the open object, the selected outputs are writable.
        let code = unsafe {
            GetSecurityInfo(
                self.handle,
                self.security_type,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &raw mut dacl,
                ptr::null_mut(),
                &raw mut descriptor,
            )
        };
        let descriptor = Local(descriptor);
        if code != 0 {
            return Err(self.error("GetSecurityInfo", Code::Win32(code), ""));
        }
        // SAFETY: a non-null DACL is backed by the live descriptor.
        if dacl.is_null() || unsafe { IsValidAcl(dacl) } == 0 {
            return Err(self.error("IsValidAcl", Code::Win32(ERROR_INVALID_ACL), ""));
        }
        Ok((descriptor, dacl))
    }
}

fn entries(
    object: &Object<'_>,
    dacl: *mut ACL,
    packages: &PackageSids<'_>,
) -> Result<Vec<(u32, PSID)>, Error> {
    let forbidden = [packages.package.0, packages.all.0, packages.restricted.0];
    // SAFETY: Object::read validated the ACL and owns its allocation during this call.
    let count = u32::from(unsafe { (*dacl).AceCount });
    let mut found = Vec::new();
    for index in 0..count {
        let mut raw = ptr::null_mut();
        // SAFETY: the index names an ACE in a live ACL.
        if unsafe { GetAce(dacl, index, &raw mut raw) } == 0 {
            return Err(object.at_object(Error::last(
                Step::StorageAcl,
                "GetAce",
                format!("entry {index}"),
            )));
        }
        // SAFETY: GetAce supplied the header of this validated entry.
        let header = unsafe { &*raw.cast::<ACE_HEADER>() };
        let kind = u32::from(header.AceType);
        let offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        let invalid = || {
            object.error(
                "GetAce",
                Code::Win32(ERROR_INVALID_ACL),
                format!("unsupported or malformed entry {index}, type {kind}"),
            )
        };
        if !matches!(
            kind,
            ACCESS_ALLOWED_ACE_TYPE
                | ACCESS_DENIED_ACE_TYPE
                | ACCESS_ALLOWED_CALLBACK_ACE_TYPE
                | ACCESS_DENIED_CALLBACK_ACE_TYPE
        ) || usize::from(header.AceSize) < offset + 8
        {
            return Err(invalid());
        }
        // SAFETY: these ACE types share this offset. The SID header fits in the ACE.
        let sid = unsafe { raw.cast::<u8>().add(offset) };
        // SAFETY: offset 1 is the subauthority count in the validated SID header.
        let count = usize::from(unsafe { *sid.add(1) });
        if offset + 8 + 4 * count > usize::from(header.AceSize) {
            return Err(invalid());
        }
        // SAFETY: the live SID and all subauthorities fit within the validated allocation.
        if let Some(target) = forbidden
            .into_iter()
            .find(|target| unsafe { EqualSid(sid.cast(), *target) } != 0)
        {
            found.push((index, target));
        }
    }
    Ok(found)
}

pub(super) fn violation(
    object: &Object<'_>,
    packages: &PackageSids<'_>,
) -> Result<Option<Error>, Error> {
    let (descriptor, dacl) = object.read()?;
    let (mut control, mut revision) = (0, 0);
    // SAFETY: descriptor is live, and control and revision are writable.
    if unsafe { GetSecurityDescriptorControl(descriptor.0, &raw mut control, &raw mut revision) }
        == 0
    {
        return Err(object.at_object(Error::last(
            Step::StorageAcl,
            "GetSecurityDescriptorControl",
            "",
        )));
    }
    let unprotected = control & SE_DACL_PROTECTED == 0;
    let matching = entries(object, dacl, packages)?;
    let detail = if let Some(&(index, sid)) = matching.first() {
        let sid = sid_text(sid).map_err(|error| {
            object.at_object(Error::launch(
                error,
                Step::StorageAcl,
                "ConvertSidToStringSidW",
            ))
        })?;
        let protection = if unprotected {
            "unprotected DACL and "
        } else {
            ""
        };
        format!("{protection}ACE {index} for package SID {sid}")
    } else if unprotected {
        "unprotected DACL".into()
    } else {
        return Ok(None);
    };
    Ok(Some(object.error(
        "GetSecurityInfo",
        Code::UnexpectedResult,
        detail,
    )))
}

pub(super) fn contains_package(
    object: &Object<'_>,
    packages: &PackageSids<'_>,
) -> Result<bool, Error> {
    let (_descriptor, dacl) = object.read()?;
    Ok(!entries(object, dacl, packages)?.is_empty())
}

pub(super) fn protect(object: &Object<'_>, packages: &PackageSids<'_>) -> Result<(), Error> {
    let (_descriptor, dacl) = object.read()?;
    for (index, _) in entries(object, dacl, packages)?.into_iter().rev() {
        // SAFETY: dacl is writable, and deleting in reverse keeps earlier indices valid.
        if unsafe { DeleteAce(dacl, index) } == 0 {
            return Err(object.at_object(Error::last(
                Step::StorageAcl,
                "DeleteAce",
                format!("entry {index}"),
            )));
        }
    }
    // SAFETY: the new DACL is live and leaves owner/group untouched.
    let code = unsafe {
        SetSecurityInfo(
            object.handle,
            object.security_type,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        )
    };
    if code != 0 {
        return Err(object.error("SetSecurityInfo", Code::Win32(code), ""));
    }
    Ok(())
}
