//! One blocking socket owner. The spawner never waits on a scope RPC.
use kinewright_job_sys::{ProcessHandle, media_host, media_scope};
use kinewright_media_ipc::{
    Frame, MediaSandboxError as Error, Message, ScopeUnavailableReason, read_frame, write_frame,
};
use std::{
    io,
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
    process::Child,
    sync::{
        Arc,
        mpsc::{self, SyncSender},
    },
    thread::JoinHandle,
    time::Duration,
};

struct Call {
    message: Message,
    reply: SyncSender<Result<Message, Error>>,
}
#[derive(Clone)]
pub(super) struct Handle {
    sender: SyncSender<Call>,
    pub pin: Arc<ProcessHandle>,
    pub generation: u64,
    pub available: bool,
}
pub(super) struct Controller {
    pub handle: Handle,
    worker: JoinHandle<()>,
}
struct Guard {
    child: Child,
    socket: UnixStream,
}
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Controller {
    /// Called only by the dedicated spawner. Bootstrap is bounded by socket
    /// timeouts; all subsequent blocking RPCs run on the controller worker.
    pub fn start(path: &Path, generation: u64) -> io::Result<Self> {
        let (socket, inherited) = media_scope::socketpair()?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        socket.set_write_timeout(Some(Duration::from_secs(2)))?;
        let child = media_host::spawn_supervisor(path, generation, &inherited)?;
        drop(inherited);
        let mut guard = Guard { child, socket };
        let pin = Arc::new(ProcessHandle::open(guard.child.id())?);
        let initial = read_frame(&mut guard.socket)?;
        if initial.request != 1 || initial.generation != generation {
            return Err(io::Error::other("supervisor bootstrap identity"));
        }
        let available = match initial.message {
            Message::Ready {} => true,
            Message::ScopeUnavailable {
                scope: None,
                reason: ScopeUnavailableReason::NoUserManager,
            } => false,
            _ => return Err(io::Error::other("supervisor bootstrap contract")),
        };
        let (sender, receiver) = mpsc::sync_channel::<Call>(4);
        let worker = std::thread::Builder::new()
            .name("media-scopes".into())
            .spawn(move || {
                let mut request = 1_u32;
                while let Ok(call) = receiver.recv() {
                    let Some(next) = request.checked_add(1) else {
                        break;
                    };
                    request = next;
                    let result = rpc(&mut guard.socket, request, generation, call.message);
                    let fatal = result.is_err();
                    // A cancelled caller still leaves a created scope under this
                    // worker's ownership until a typed Stop reply completes.
                    let abandoned = match &result {
                        Ok(
                            Message::ScopeCreated { scope }
                            | Message::ScopeCreateFailed { scope, .. }
                            | Message::ScopeUnavailable {
                                scope: Some(scope), ..
                            },
                        ) => Some(*scope),
                        _ => None,
                    };
                    if call.reply.send(result).is_err()
                        && let Some(scope) = abandoned
                    {
                        let Some(next) = request.checked_add(1) else {
                            break;
                        };
                        request = next;
                        if rpc(
                            &mut guard.socket,
                            request,
                            generation,
                            Message::StopScope { scope },
                        )
                        .is_err()
                        {
                            break;
                        }
                    }
                    if fatal {
                        break;
                    }
                }
            })?;
        Ok(Self {
            handle: Handle {
                sender,
                pin,
                generation,
                available,
            },
            worker,
        })
    }
    pub fn finished(&self) -> bool {
        self.worker.is_finished()
    }
    pub fn shutdown(self) {
        drop(self.handle);
        let _ = self.worker.join();
    }
}
impl Handle {
    pub fn call(&self, message: Message) -> Result<Message, Error> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .try_send(Call { message, reply })
            .map_err(|_| Error::Crashed)?;
        receiver
            .recv_timeout(Duration::from_secs(6))
            .map_err(|_| Error::TimedOut)?
    }
}
fn rpc(
    socket: &mut UnixStream,
    request: u32,
    generation: u64,
    message: Message,
) -> Result<Message, Error> {
    write_frame(
        socket,
        &Frame {
            request,
            generation,
            message,
        },
    )
    .map_err(|_| Error::Crashed)?;
    let frame = read_frame(socket).map_err(|_| Error::ProtocolViolation)?;
    if frame.request != request || frame.generation != generation {
        return Err(Error::ProtocolViolation);
    }
    match frame.message {
        message @ (Message::ScopeCreated { .. }
        | Message::ScopeVerified { .. }
        | Message::ScopeStopped { .. }
        | Message::ScopeCreateFailed { .. }
        | Message::ScopeUnavailable { .. }
        | Message::Failed { .. }) => Ok(message),
        _ => Err(Error::ProtocolViolation),
    }
}
