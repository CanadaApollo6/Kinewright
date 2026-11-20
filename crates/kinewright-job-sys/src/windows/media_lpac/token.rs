use super::{Handle, Sid, check_step, last_error, read_token, token_buffer, wide};
use std::{ffi::OsStr, io, mem::size_of, ptr};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        AccessCheck,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        DuplicateTokenEx, EqualSid, GENERIC_MAPPING, GetSidSubAuthority, GetSidSubAuthorityCount,
        PRIVILEGE_SET, PSID, SecurityIdentification, TOKEN_APPCONTAINER_INFORMATION,
        TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER, TokenAppContainerSid,
        TokenCapabilities, TokenImpersonation, TokenIntegrityLevel, TokenIsAppContainer, TokenUser,
    },
    System::{SystemServices::SECURITY_MANDATORY_LOW_RID, Threading::OpenProcessToken},
};

pub(super) fn open(process: &Handle, rights: u32) -> io::Result<Handle> {
    let mut token = ptr::null_mut();
    // SAFETY: process is live and token receives an owned handle.
    check_step("OpenProcessToken (suspended AppContainer)", unsafe {
        OpenProcessToken(process.0, rights, &raw mut token)
    })?;
    Ok(Handle(token))
}

pub(super) fn inspect_token(process: &Handle, expected: &Sid) -> io::Result<()> {
    use super::expect_value;
    let token = open(process, TOKEN_QUERY)?;
    expect_value(
        "GetTokenInformation TokenIsAppContainer",
        query_app_container(&token)?,
        1,
    )?;
    expect_value(
        "GetTokenInformation TokenAppContainerSid",
        query_sid(&token, expected)?,
        true,
    )?;
    expect_value(
        "GetTokenInformation TokenCapabilities",
        query_capabilities(&token)?,
        0,
    )?;
    expect_value(
        "GetTokenInformation TokenIntegrityLevel",
        query_integrity(&token)?,
        SECURITY_MANDATORY_LOW_RID,
    )?;
    let (own, all_packages) = package_access(process, &token, expected)?;
    expect_value("AccessCheck own package SID read", own?, true)?;
    expect_value(
        "AccessCheck ALL APPLICATION PACKAGES read",
        all_packages?,
        false,
    )
}

fn query_app_container(token: &Handle) -> io::Result<u32> {
    let mut value = 0;
    read_token(
        token,
        TokenIsAppContainer,
        "TokenIsAppContainer",
        &mut value,
    )?;
    Ok(value)
}

pub(super) fn query_sid(token: &Handle, expected: &Sid) -> io::Result<bool> {
    let storage = token_buffer(
        token,
        TokenAppContainerSid,
        "TokenAppContainerSid",
        size_of::<TOKEN_APPCONTAINER_INFORMATION>(),
    )?;
    // SAFETY: the buffer holds the aligned SID information and retains its SID.
    Ok(unsafe {
        let sid = (*storage.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>()).TokenAppContainer;
        !sid.is_null() && EqualSid(sid, expected.0) != 0
    })
}

fn query_capabilities(token: &Handle) -> io::Result<u32> {
    let storage = token_buffer(
        token,
        TokenCapabilities,
        "TokenCapabilities",
        size_of::<u32>(),
    )?;
    // SAFETY: TOKEN_GROUPS starts with a DWORD count.
    Ok(unsafe { *storage.as_ptr().cast::<u32>() })
}

fn query_integrity(token: &Handle) -> io::Result<i32> {
    let storage = token_buffer(
        token,
        TokenIntegrityLevel,
        "TokenIntegrityLevel",
        size_of::<TOKEN_MANDATORY_LABEL>(),
    )?;
    // SAFETY: the successful query populated the aligned label and its SID.
    unsafe {
        let sid = (*storage.as_ptr().cast::<TOKEN_MANDATORY_LABEL>())
            .Label
            .Sid;
        if sid.is_null() {
            return Err(io::Error::other(
                "GetTokenInformation Win32 0 returned a null label SID",
            ));
        }
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return Err(io::Error::other(
                "GetTokenInformation Win32 0 returned a label without a RID",
            ));
        }
        Ok((*GetSidSubAuthority(sid, u32::from(count) - 1)).cast_signed())
    }
}

pub(super) struct Local(pub(super) *mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: this owns a LocalAlloc pointer returned by the security APIs.
        unsafe { LocalFree(self.0) };
    }
}

pub(super) fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = ptr::null_mut();
    // SAFETY: callers keep the SID allocation live and text is writable.
    check_step("ConvertSidToStringSidW (AccessCheck)", unsafe {
        ConvertSidToStringSidW(sid, &raw mut text)
    })?;
    let _handle = Local(text.cast());
    // SAFETY: the conversion returned a terminated string owned by the guard.
    Ok(unsafe { super::super::copy_os_string(text) }
        .to_string_lossy()
        .into_owned())
}

type AccessResults = (io::Result<bool>, io::Result<bool>);

fn package_access(process: &Handle, token: &Handle, package: &Sid) -> io::Result<AccessResults> {
    let user = token_buffer(token, TokenUser, "TokenUser", size_of::<TOKEN_USER>())?;
    // SAFETY: the successful token query populated an aligned TOKEN_USER and its SID.
    let user = sid_text(unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid })?;
    let package = sid_text(package.0)?;
    let source = open(process, TOKEN_DUPLICATE)?;
    let mut duplicate = ptr::null_mut();
    // SAFETY: source is live, duplicate is writable, and the copy is only used in AccessCheck.
    check_step("DuplicateTokenEx (AccessCheck)", unsafe {
        DuplicateTokenEx(
            source.0,
            TOKEN_QUERY,
            ptr::null(),
            SecurityIdentification,
            TokenImpersonation,
            &raw mut duplicate,
        )
    })?;
    let duplicate = Handle(duplicate);
    Ok((
        check_read(&duplicate, &user, &package),
        check_read(&duplicate, &user, "S-1-15-2-1"),
    ))
}

fn check_read(token: &Handle, user: &str, package: &str) -> io::Result<bool> {
    let text = wide(OsStr::new(&format!(
        "O:SYG:SYD:(A;;0x1;;;{user})(A;;0x1;;;{package})"
    )))?;
    let mut descriptor = ptr::null_mut();
    // SAFETY: valid generated SDDL is terminated and descriptor receives an owned allocation.
    check_step(
        "ConvertStringSecurityDescriptorToSecurityDescriptorW (AccessCheck)",
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                ptr::null_mut(),
            )
        },
    )?;
    let descriptor = Local(descriptor);
    let mapping = GENERIC_MAPPING {
        GenericRead: 1,
        GenericWrite: 2,
        GenericExecute: 4,
        GenericAll: 7,
    };
    let mut privileges = PRIVILEGE_SET::default();
    let mut size = size_of::<PRIVILEGE_SET>()
        .try_into()
        .expect("privilege buffer size");
    let (mut granted, mut allowed) = (0, 0);
    // SAFETY: the owned descriptor and identification token are live, all outputs are writable.
    let ok = unsafe {
        AccessCheck(
            descriptor.0,
            token.0,
            1,
            &raw const mapping,
            &raw mut privileges,
            &raw mut size,
            &raw mut granted,
            &raw mut allowed,
        )
    };
    if ok == 0 {
        return Err(last_error("AccessCheck (package-only read)"));
    }
    Ok(allowed != 0 && granted & 1 != 0)
}
