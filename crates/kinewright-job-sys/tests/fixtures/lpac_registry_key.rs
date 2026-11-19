use std::{ffi::OsStr, mem::size_of, os::windows::ffi::OsStrExt, ptr};
use windows_sys::{
    Wdk::{Foundation::OBJECT_ATTRIBUTES, System::Registry::NtOpenKey},
    Win32::{
        Foundation::{OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, UNICODE_STRING},
        System::Registry::{HKEY, RegCloseKey},
    },
};

pub(super) struct RegistryKey(pub(super) HKEY);
impl Drop for RegistryKey {
    fn drop(&mut self) {
        // SAFETY: NtOpenKey transferred this registry handle to this owner.
        unsafe { RegCloseKey(self.0) };
    }
}

pub(super) struct RegistryError {
    api: &'static str,
    status: Option<i32>,
    pub(super) code: u32,
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

pub(super) fn open_registry(name: &str, rights: u32) -> Result<RegistryKey, RegistryError> {
    let mut units: Vec<u16> = OsStr::new(name).encode_wide().chain([0]).collect();
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
