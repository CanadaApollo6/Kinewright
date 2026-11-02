//! C5 fix 8: one connected descriptor crosses a private Unix channel. The
//! unpaid dry policy's namespace init hands each loopback connection it
//! accepts to the host this way; neither side copies bytes through init.
use std::{
    io::{self, IoSlice, IoSliceMut},
    mem::MaybeUninit,
    os::{
        fd::{AsFd, OwnedFd},
        unix::net::UnixStream,
    },
};

const MARKER: u8 = b'R';

/// Send one descriptor with a one-byte marker.
///
/// # Errors
/// Refuses a closed or failed channel.
pub fn send_descriptor(channel: &UnixStream, descriptor: impl AsFd) -> io::Result<()> {
    use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    let descriptors = [descriptor.as_fd()];
    if !control.push(SendAncillaryMessage::ScmRights(&descriptors)) {
        return Err(io::Error::other("descriptor control capacity"));
    }
    let sent = sendmsg(
        channel,
        &[IoSlice::new(&[MARKER])],
        &mut control,
        SendFlags::NOSIGNAL,
    )?;
    if sent != 1 {
        return Err(io::Error::other("descriptor marker short"));
    }
    Ok(())
}

/// Receive one descriptor, close-on-exec. `None` at the channel's end.
///
/// # Errors
/// Refuses a message without exactly one descriptor, or a failed channel.
pub fn receive_descriptor(channel: &UnixStream) -> io::Result<Option<OwnedFd>> {
    use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    let mut marker = [0];
    let received = loop {
        match recvmsg(
            channel,
            &mut [IoSliceMut::new(&mut marker)],
            &mut control,
            RecvFlags::CMSG_CLOEXEC,
        ) {
            Err(rustix::io::Errno::INTR) => {}
            other => break other?,
        }
    };
    let mut descriptors = Vec::new();
    for message in control.drain() {
        if let RecvAncillaryMessage::ScmRights(received) = message {
            descriptors.extend(received);
        }
    }
    if received.bytes == 0 && descriptors.is_empty() {
        return Ok(None);
    }
    if received.bytes != 1
        || marker != [MARKER]
        || descriptors.len() != 1
        || received.flags.contains(rustix::net::ReturnFlags::CTRUNC)
    {
        return Err(io::Error::other("descriptor message refused"));
    }
    Ok(descriptors.pop())
}

#[cfg(test)]
mod tests {
    #[test]
    fn s4b_c5_fix8_descriptor_crosses_channel() {
        use std::io::{Read, Write};
        let (left, right) = std::os::unix::net::UnixStream::pair().unwrap();
        let (mut near, far) = std::os::unix::net::UnixStream::pair().unwrap();
        super::send_descriptor(&left, &far).unwrap();
        drop(far);
        let mut far = std::os::unix::net::UnixStream::from(
            super::receive_descriptor(&right).unwrap().unwrap(),
        );
        near.write_all(b"ping").unwrap();
        let mut bytes = [0; 4];
        far.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ping");
        drop(left);
        assert!(super::receive_descriptor(&right).unwrap().is_none());
    }
}
