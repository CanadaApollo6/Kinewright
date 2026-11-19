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

pub(super) struct Inspection {
    #[cfg(feature = "lpac-diagnostics")]
    pub(super) base_verified: bool,
    pub(super) verified: bool,
    pub(super) report: String,
}

impl Inspection {
    pub(super) fn require_verified(self) -> io::Result<()> {
        if self.verified {
            #[cfg(feature = "lpac-diagnostics")]
            eprintln!("suspended token: {}", self.report);
            Ok(())
        } else {
            Err(io::Error::other(self.report))
        }
    }
}

pub(super) fn open(process: &Handle, rights: u32) -> io::Result<Handle> {
    let mut token = ptr::null_mut();
    // SAFETY: process is live and token receives an owned handle.
    check_step("OpenProcessToken (suspended AppContainer)", unsafe {
        OpenProcessToken(process.0, rights, &raw mut token)
    })?;
    Ok(Handle(token))
}

pub(super) fn inspect_token(process: &Handle, expected: &Sid, lpac: bool) -> Inspection {
    let token = match open(process, TOKEN_QUERY) {
        Ok(token) => token,
        Err(error) => {
            return Inspection {
                #[cfg(feature = "lpac-diagnostics")]
                base_verified: false,
                verified: false,
                report: format!(
                    "{error}; TokenIsAppContainer, TokenAppContainerSid, TokenCapabilities, TokenIntegrityLevel and LPAC unconfirmed"
                ),
            };
        }
    };
    let app_container = query_app_container(&token);
    let sid = query_sid(&token, expected);
    let capabilities = query_capabilities(&token);
    let integrity = query_integrity(&token);
    let (own_access, access) = match package_access(process, &token, expected) {
        Ok(checks) => checks,
        Err(err) => (Err(io::Error::other(err.to_string())), Err(err)),
    };
    let base_verified = app_container.as_ref().is_ok_and(|value| *value == 1)
        && sid.as_ref().is_ok_and(|matched| *matched)
        && capabilities.as_ref().is_ok_and(|count| *count == 0)
        && integrity
            .as_ref()
            .is_ok_and(|rid| *rid == SECURITY_MANDATORY_LOW_RID);
    let verified = base_verified
        && own_access.as_ref().is_ok_and(|allowed| *allowed)
        && access.as_ref().is_ok_and(|allowed| *allowed != lpac);
    let facts = [
        fact("GetTokenInformation TokenIsAppContainer", app_container, 1),
        fact(
            "GetTokenInformation TokenAppContainerSid matches",
            sid,
            true,
        ),
        fact(
            "GetTokenInformation TokenCapabilities count",
            capabilities,
            0,
        ),
        fact(
            "GetTokenInformation TokenIntegrityLevel RID",
            integrity,
            SECURITY_MANDATORY_LOW_RID,
        ),
        fact("AccessCheck own package SID read", own_access, true),
        fact("AccessCheck ALL APPLICATION PACKAGES read", access, !lpac),
    ];
    #[cfg(feature = "lpac-diagnostics")]
    let facts = facts
        .into_iter()
        .chain(super::token_queries::diagnose(&token))
        .collect::<Vec<_>>();
    // Keep the same facts on successful and failed startup; a query error never masks another.
    let report = facts.join(" | ");
    Inspection {
        #[cfg(feature = "lpac-diagnostics")]
        base_verified,
        verified,
        report,
    }
}

fn fact<T: std::fmt::Debug>(name: &str, value: io::Result<T>, expected: T) -> String {
    match value {
        Ok(value) => format!("{name} Win32 0 observed={value:?} expected={expected:?}"),
        Err(error) => format!("{name} unconfirmed ({error}) expected={expected:?}"),
    }
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

fn query_sid(token: &Handle, expected: &Sid) -> io::Result<bool> {
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

struct Local(*mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: this owns a LocalAlloc pointer returned by the security APIs.
        unsafe { LocalFree(self.0) };
    }
}

fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = ptr::null_mut();
    // SAFETY: sid belongs to a live token buffer, text is writable.
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
