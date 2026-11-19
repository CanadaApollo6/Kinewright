use std::{
    ffi::OsStr,
    io::{self, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    ptr,
};
use windows_sys::Win32::{
    Foundation::GetLastError,
    Security::{Isolation::GetAppContainerNamedObjectPath, TOKEN_QUERY},
    System::Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken, SetEvent},
};

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

fn last_error() -> u32 {
    // SAFETY: read this thread's last error after a failed Win32 call.
    unsafe { GetLastError() }
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
