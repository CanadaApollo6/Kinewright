//! C5 fix 9: the raw socket calls the unpaid witness needs and the safe
//! crates do not expose: a socket of any family, and `AF_VSOCK` addresses.
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

/// `VMADDR_CID_LOCAL`: this machine's own VSOCK loopback.
pub const VSOCK_LOCAL: u32 = 1;
const VSOCK_ANY: u32 = u32::MAX;

#[repr(C)]
struct VsockAddress {
    family: libc::sa_family_t,
    reserved: u16,
    port: u32,
    cid: u32,
    flags: u8,
    zero: [u8; 3],
}
impl VsockAddress {
    fn new(cid: u32, port: u32) -> Self {
        Self {
            family: libc::sa_family_t::try_from(libc::AF_VSOCK).unwrap_or_default(),
            reserved: 0,
            port,
            cid,
            flags: 0,
            zero: [0; 3],
        }
    }
}
const VSOCK_LENGTH: libc::socklen_t = 16;
const _: () = assert!(size_of::<VsockAddress>() == VSOCK_LENGTH as usize);

/// Open a close-on-exec socket of any family and type.
///
/// # Errors
/// Returns the kernel's or the seccomp filter's refusal.
pub fn open_socket(family: i32, kind: i32, protocol: i32) -> io::Result<OwnedFd> {
    // SAFETY: socket(2) takes integers only; a non-negative result is a new
    // descriptor this call owns.
    let descriptor = unsafe { libc::socket(family, kind | libc::SOCK_CLOEXEC, protocol) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor was just returned by socket(2) and is owned here.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

/// Connect a VSOCK stream to `cid:port`.
///
/// # Errors
/// Returns the socket or connect refusal.
pub fn vsock_connect(cid: u32, port: u32) -> io::Result<OwnedFd> {
    let socket = open_socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0)?;
    let address = VsockAddress::new(cid, port);
    // SAFETY: `address` is a live, correctly sized `sockaddr_vm`.
    let result = unsafe {
        libc::connect(
            socket.as_raw_fd(),
            (&raw const address).cast(),
            VSOCK_LENGTH,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(socket)
}

/// A non-blocking VSOCK listener on any CID and a kernel-chosen port.
pub struct VsockListener {
    socket: OwnedFd,
    pub port: u32,
}
impl VsockListener {
    /// # Errors
    /// Refuses when this machine has no VSOCK transport.
    pub fn bind() -> io::Result<Self> {
        let socket = open_socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_NONBLOCK, 0)?;
        let mut address = VsockAddress::new(VSOCK_ANY, VSOCK_ANY);
        let mut length = VSOCK_LENGTH;
        // SAFETY: `address` is a live, correctly sized `sockaddr_vm`, and
        // `length` a live length for getsockname(2).
        let result = unsafe {
            if libc::bind(
                socket.as_raw_fd(),
                (&raw const address).cast(),
                VSOCK_LENGTH,
            ) < 0
                || libc::listen(socket.as_raw_fd(), 16) < 0
            {
                -1
            } else {
                libc::getsockname(
                    socket.as_raw_fd(),
                    (&raw mut address).cast(),
                    &raw mut length,
                )
            }
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            socket,
            port: address.port,
        })
    }
    /// Accept and close every pending connection; returns how many there were.
    #[must_use]
    pub fn drain(&self) -> usize {
        let mut accepted = 0;
        loop {
            // SAFETY: accept4(2) with null address pointers; a non-negative
            // result is a new descriptor owned here and closed at once.
            let descriptor = unsafe {
                libc::accept4(
                    self.socket.as_raw_fd(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_CLOEXEC,
                )
            };
            if descriptor < 0 {
                return accepted;
            }
            // SAFETY: the descriptor was just accepted and is owned here.
            drop(unsafe { OwnedFd::from_raw_fd(descriptor) });
            accepted += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn s4b_c5_fix9_vsock_loopback_reaches_the_listener() {
        let Ok(listener) = super::VsockListener::bind() else {
            eprintln!("C5_FIX9_NO_VSOCK_TRANSPORT");
            return;
        };
        let connected = super::vsock_connect(super::VSOCK_LOCAL, listener.port);
        if connected.is_err() {
            eprintln!("C5_FIX9_NO_VSOCK_LOOPBACK {connected:?}");
            return;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut seen = 0;
        while seen == 0 && std::time::Instant::now() < deadline {
            seen = listener.drain();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(seen, 1, "C5_FIX9_VSOCK_LISTENER_MISSED");
    }
}
