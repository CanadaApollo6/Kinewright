// Win32 reserves TokenSecurityAttributes.
// Keep both the native shape and its marker outside the required inspection.
use super::{Handle, token_buffer};
use std::{mem::size_of, ptr};
use windows_sys::{
    Wdk::Storage::FileSystem::NtQueryInformationToken,
    Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, RtlNtStatusToDosError, UNICODE_STRING},
        Security::TokenSecurityAttributes,
    },
};

#[repr(C)]
struct Attributes {
    version: u16,
    reserved: u16,
    count: u32,
    entries: *const Attribute,
}

#[repr(C)]
struct Attribute {
    name: UNICODE_STRING,
    value_type: u16,
    reserved: u16,
    flags: u32,
    count: u32,
    values: *const u64,
}

pub(super) fn diagnose(token: &Handle) -> [String; 2] {
    let name = "GetTokenInformation TokenSecurityAttributes";
    let win32 = match token_buffer(
        token,
        TokenSecurityAttributes,
        "TokenSecurityAttributes",
        size_of::<Attributes>(),
    ) {
        Ok(buffer) => format!("{name} Win32 0 {}", marker(&buffer)),
        Err(error) => format!("{name}: {error}"),
    };
    [win32, native_attributes(token)]
}

fn native_status(status: i32) -> String {
    // SAFETY: this conversion only takes a numeric NTSTATUS.
    let code = if status >= 0 {
        0
    } else {
        unsafe { RtlNtStatusToDosError(status) }
    };
    format!("Win32 {code} NTSTATUS {:#010x}", status.cast_unsigned())
}

fn native_attributes(token: &Handle) -> String {
    let name = "NtQueryInformationToken TokenSecurityAttributes";
    let mut size = 0;
    // SAFETY: null buffer and writable size ask the API for its required size.
    let status = unsafe {
        NtQueryInformationToken(
            token.0,
            TokenSecurityAttributes,
            ptr::null_mut(),
            0,
            &raw mut size,
        )
    };
    // SAFETY: this conversion only takes a numeric NTSTATUS.
    let code = unsafe { RtlNtStatusToDosError(status) };
    if code != ERROR_INSUFFICIENT_BUFFER
        || !(size_of::<Attributes>()..=65536).contains(&(size as usize))
    {
        return format!(
            "{name} size query {} returned={size}",
            native_status(status)
        );
    }
    let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    let mut returned = 0;
    // SAFETY: the aligned buffer and output length are writable for the declared size.
    let status = unsafe {
        NtQueryInformationToken(
            token.0,
            TokenSecurityAttributes,
            buffer.as_mut_ptr().cast(),
            size,
            &raw mut returned,
        )
    };
    let result = if status >= 0
        && (size_of::<Attributes>()..=size as usize).contains(&(returned as usize))
    {
        marker(&buffer)
    } else {
        "marker=unconfirmed".into()
    };
    format!(
        "{name} {} returned={returned} {result}",
        native_status(status)
    )
}

fn within<T>(buffer: &[usize], ptr: *const T, count: usize) -> Option<&[T]> {
    let start = buffer.as_ptr().addr();
    let offset = ptr.addr().checked_sub(start)?;
    let end = offset.checked_add(size_of::<T>().checked_mul(count)?)?;
    if end > size_of_val(buffer) || !ptr.is_aligned() {
        return None;
    }
    // SAFETY: verified aligned range is within initialized buffer storage. Use its provenance.
    Some(unsafe {
        std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>().add(offset).cast(), count)
    })
}

fn marker(buffer: &[usize]) -> String {
    const NAME: &str = "WIN://NOALLAPPPKG";
    let Some([header]) = within(buffer, buffer.as_ptr().cast::<Attributes>(), 1) else {
        return format!("{NAME}=unconfirmed (short header)");
    };
    if header.version != 1 {
        return format!("{NAME}=unconfirmed (attribute version {})", header.version);
    }
    if header.count == 0 {
        return format!("{NAME}=absent");
    }
    let Some(entries) = within(buffer, header.entries, header.count as usize) else {
        return format!("{NAME}=unconfirmed (invalid attribute list)");
    };
    for entry in entries {
        let length = entry.name.Length;
        if length % 2 != 0 || length > entry.name.MaximumLength {
            return format!("{NAME}=unconfirmed (invalid name length)");
        }
        let Some(name) = within(buffer, entry.name.Buffer, usize::from(length / 2)) else {
            return format!("{NAME}=unconfirmed (invalid name pointer)");
        };
        if String::from_utf16_lossy(name).eq_ignore_ascii_case(NAME) {
            let value = (entry.value_type == 2 && entry.count == 1)
                .then(|| within(buffer, entry.values, 1).map(|values| values[0]))
                .flatten();
            return format!(
                "{NAME}=present type={} count={} uint64={value:?}",
                entry.value_type, entry.count
            );
        }
    }
    format!("{NAME}=absent")
}
