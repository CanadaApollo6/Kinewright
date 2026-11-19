use super::{Handle, check_step, expect_value, token, token_buffer, wide};
use std::{ffi::OsStr, io, mem::offset_of, ptr};
use windows_sys::Win32::{
    Foundation::GENERIC_ALL,
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
            SE_KERNEL_OBJECT,
        },
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, IsValidAcl, IsValidSid,
        OWNER_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES,
        SetTokenInformation, TOKEN_ADJUST_DEFAULT, TOKEN_DEFAULT_DACL, TOKEN_QUERY,
        TokenDefaultDacl,
    },
    System::{
        SystemServices::ACCESS_ALLOWED_ACE_TYPE,
        Threading::{PROCESS_ALL_ACCESS, THREAD_ALL_ACCESS},
    },
};

pub(super) struct ChildSecurity {
    descriptor: token::Local,
    owner: PSID,
    dacl: *mut ACL,
}

impl ChildSecurity {
    pub(super) fn new() -> io::Result<Self> {
        let host = super::super::account_sid()?;
        let text = wide(OsStr::new(&format!(
            "O:{host}D:P(A;;GA;;;{host})(A;;GA;;;SY)"
        )))?;
        let mut descriptor = token::Local(ptr::null_mut());
        // SAFETY: the generated SDDL is terminated and the descriptor output is writable.
        check_step(
            "ConvertStringSecurityDescriptorToSecurityDescriptorW (helper)",
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    SDDL_REVISION_1,
                    &raw mut descriptor.0,
                    ptr::null_mut(),
                )
            },
        )?;
        let (mut owner, mut dacl) = (ptr::null_mut(), ptr::null_mut());
        let (mut present, mut defaulted) = (0, 0);
        // SAFETY: descriptor is live and the selected outputs are writable.
        check_step("GetSecurityDescriptorOwner (helper template)", unsafe {
            GetSecurityDescriptorOwner(descriptor.0, &raw mut owner, &raw mut defaulted)
        })?;
        // SAFETY: descriptor is live and all outputs are writable.
        check_step("GetSecurityDescriptorDacl (helper template)", unsafe {
            GetSecurityDescriptorDacl(
                descriptor.0,
                &raw mut present,
                &raw mut dacl,
                &raw mut defaulted,
            )
        })?;
        expect_value(
            "helper security descriptor owner and DACL are present",
            !owner.is_null() && present != 0 && valid_pair(dacl),
            true,
        )?;
        Ok(Self {
            descriptor,
            owner,
            dacl,
        })
    }

    pub(super) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
                .expect("security attributes size"),
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: 0,
        }
    }

    pub(super) fn inspect_and_set_defaults(
        &self,
        process: &Handle,
        thread: &Handle,
    ) -> io::Result<()> {
        self.inspect_object(process, "helper process descriptor", PROCESS_ALL_ACCESS)?;
        self.inspect_object(
            thread,
            "helper primary thread descriptor",
            THREAD_ALL_ACCESS,
        )?;

        let token = token::open(process, TOKEN_QUERY | TOKEN_ADJUST_DEFAULT)?;
        let default = TOKEN_DEFAULT_DACL {
            DefaultDacl: self.dacl,
        };
        // SAFETY: the suspended child's token is live and default points to our live ACL.
        check_step("SetTokenInformation(TokenDefaultDacl)", unsafe {
            SetTokenInformation(
                token.0,
                TokenDefaultDacl,
                (&raw const default).cast(),
                u32::try_from(size_of::<TOKEN_DEFAULT_DACL>()).expect("token default DACL size"),
            )
        })?;
        let storage = token_buffer(
            &token,
            TokenDefaultDacl,
            "TokenDefaultDacl",
            size_of::<TOKEN_DEFAULT_DACL>(),
        )?;
        // SAFETY: the successful token query returned the aligned structure and its live ACL.
        let observed = unsafe { (*storage.as_ptr().cast::<TOKEN_DEFAULT_DACL>()).DefaultDacl };
        self.inspect_dacl(
            observed,
            "GetTokenInformation(TokenDefaultDacl) host and SYSTEM only",
            GENERIC_ALL,
        )
    }

    fn inspect_object(&self, handle: &Handle, name: &str, mapped_all: u32) -> io::Result<()> {
        let (mut owner, mut dacl) = (ptr::null_mut(), ptr::null_mut());
        let mut descriptor = token::Local(ptr::null_mut());
        // SAFETY: the suspended object is live and all selected outputs are writable.
        let code = unsafe {
            GetSecurityInfo(
                handle.0,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &raw mut owner,
                ptr::null_mut(),
                &raw mut dacl,
                ptr::null_mut(),
                &raw mut descriptor.0,
            )
        };
        if code != 0 {
            return Err(super::os_error(
                &format!("GetSecurityInfo ({name})"),
                &io::Error::from_raw_os_error(code.cast_signed()),
            ));
        }
        let (mut control, mut revision) = (0, 0);
        // SAFETY: GetSecurityInfo supplied a live descriptor and the outputs are writable.
        check_step(&format!("GetSecurityDescriptorControl ({name})"), unsafe {
            GetSecurityDescriptorControl(descriptor.0, &raw mut control, &raw mut revision)
        })?;
        // SAFETY: non-null owner pointers are backed by the two live security descriptors.
        let host_owns = !owner.is_null() && unsafe { EqualSid(owner, self.owner) } != 0;
        expect_value(&format!("{name} host owner"), host_owns, true)?;
        expect_value(
            &format!("{name} protected DACL"),
            control & SE_DACL_PROTECTED != 0,
            true,
        )?;
        self.inspect_dacl(dacl, &format!("{name} host and SYSTEM only"), mapped_all)
    }

    fn inspect_dacl(&self, observed: *mut ACL, step: &str, mapped_all: u32) -> io::Result<()> {
        expect_value(step, valid_pair(observed), true)?;
        let expected = [
            allowed_entry(self.dacl, 0, step)?,
            allowed_entry(self.dacl, 1, step)?,
        ];
        let actual = [
            allowed_entry(observed, 0, step)?,
            allowed_entry(observed, 1, step)?,
        ];
        let rights_ok = actual
            .iter()
            .all(|&(mask, _)| mask == GENERIC_ALL || mask == mapped_all);
        let matches = |first: usize, second: usize| {
            // SAFETY: all SIDs were validated within their live ACLs.
            unsafe {
                EqualSid(actual[0].1, expected[first].1) != 0
                    && EqualSid(actual[1].1, expected[second].1) != 0
            }
        };
        expect_value(step, rights_ok && (matches(0, 1) || matches(1, 0)), true)
    }
}

fn valid_pair(dacl: *mut ACL) -> bool {
    // SAFETY: non-null pointers come from a live descriptor or token query.
    !dacl.is_null() && unsafe { IsValidAcl(dacl) != 0 && (*dacl).AceCount == 2 }
}

fn allowed_entry(dacl: *mut ACL, index: u32, step: &str) -> io::Result<(u32, PSID)> {
    let mut raw = ptr::null_mut();
    // SAFETY: the DACL was validated and raw is writable.
    check_step(&format!("GetAce ({step})"), unsafe {
        GetAce(dacl, index, &raw mut raw)
    })?;
    // SAFETY: GetAce supplied a live header in the validated DACL.
    let header = unsafe { &*raw.cast::<ACE_HEADER>() };
    let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    let size = usize::from(header.AceSize);
    expect_value(
        step,
        u32::from(header.AceType) == ACCESS_ALLOWED_ACE_TYPE
            && header.AceFlags == 0
            && size >= sid_offset + 8,
        true,
    )?;
    // SAFETY: the checked ACE contains a SID header and the preceding access mask.
    let sid = unsafe { raw.cast::<u8>().add(sid_offset) };
    // SAFETY: the subauthority count is at offset 1 of the checked SID header.
    let count = usize::from(unsafe { *sid.add(1) });
    expect_value(step, size == sid_offset + 8 + count * 4, true)?;
    // SAFETY: the entire SID is in bounds for validation.
    expect_value(step, unsafe { IsValidSid(sid.cast()) } != 0, true)?;
    // SAFETY: the validated ACE has a DWORD access mask and all SID bytes in bounds.
    Ok((
        unsafe { (*raw.cast::<ACCESS_ALLOWED_ACE>()).Mask },
        sid.cast(),
    ))
}
