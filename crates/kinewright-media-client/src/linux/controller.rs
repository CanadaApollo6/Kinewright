//! One blocking socket owner. The spawner never waits on a scope RPC.
use super::{Phase, Unavailable, UnavailableReason as Reason};
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
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::JoinHandle,
    time::Duration,
};

enum Call {
    Request {
        message: Message,
        reply: SyncSender<Result<Message, Error>>,
    },
    ReleaseScope(u32),
}
#[derive(Clone)]
pub(super) struct Handle {
    sender: SyncSender<Call>,
    pub pin: Arc<ProcessHandle>,
    pub generation: u64,
    pub available: bool,
    pending_stops: Arc<AtomicUsize>,
}
pub(super) struct Controller {
    pub handle: Handle,
    worker: JoinHandle<Result<(), Error>>,
}
struct Guard {
    child: Child,
    socket: UnixStream,
}
impl Guard {
    fn finish(&self, pin: &ProcessHandle) {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let _ = self.socket.shutdown(Shutdown::Both);
        // Let the supervisor stop residual scope units on socket EOF. The
        // guard will still kill and reap it if the bus stalls again.
        let timeout = Timespec {
            tv_sec: 2,
            tv_nsec: 0,
        };
        let _ = poll(&mut [PollFd::new(pin, PollFlags::IN)], Some(&timeout));
    }
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
    pub fn start(path: &Path, generation: u64) -> Result<Self, Unavailable> {
        let error = |reason| move |_| Phase::Supervisor.unavailable(reason);
        let (socket, inherited) =
            media_scope::socketpair().map_err(error(Reason::SupervisorSocket))?;
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(error(Reason::SupervisorSocket))?;
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(error(Reason::SupervisorSocket))?;
        let child = media_host::spawn_supervisor(path, generation, &inherited)
            .map_err(error(Reason::SupervisorSpawn))?;
        drop(inherited);
        let mut guard = Guard { child, socket };
        let pin =
            Arc::new(ProcessHandle::open(guard.child.id()).map_err(error(Reason::SupervisorPin))?);
        let initial = read_frame(&mut guard.socket).map_err(|error| {
            let reason = match error.kind() {
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
                    Reason::SupervisorHandshakeTimedOut
                }
                _ => Reason::SupervisorHandshake,
            };
            Phase::Supervisor.unavailable(reason)
        })?;
        if initial.request != 1 || initial.generation != generation {
            return Err(Phase::Supervisor.unavailable(Reason::SupervisorHandshake));
        }
        let available = match initial.message {
            Message::Ready {} => true,
            Message::ScopeUnavailable {
                scope: None,
                reason: ScopeUnavailableReason::NoUserManager,
            } => false,
            _ => return Err(Phase::Supervisor.unavailable(Reason::SupervisorHandshake)),
        };
        let (sender, receiver) = mpsc::sync_channel::<Call>(4);
        // The worker accounts for stop acknowledgements while the spawner
        // observes whether this supervisor can accept another scope.
        let pending_stops = Arc::new(AtomicUsize::new(0));
        let worker_stops = Arc::clone(&pending_stops);
        let worker_pin = Arc::clone(&pin);
        let worker = std::thread::Builder::new()
            .name("media-scopes".into())
            .spawn(move || {
                let result = serve(&mut guard.socket, generation, &receiver, &worker_stops);
                guard.finish(&worker_pin);
                result
            })
            .map_err(error(Reason::SupervisorThread))?;
        Ok(Self {
            handle: Handle {
                sender,
                pin,
                generation,
                available,
                pending_stops,
            },
            worker,
        })
    }
    pub fn finished(&self) -> bool {
        self.worker.is_finished()
    }
    pub fn scope_cleanup_pending(&self) -> bool {
        self.handle.pending_stops.load(Ordering::Acquire) != 0
    }
    pub fn shutdown(self) -> Result<(), Error> {
        drop(self.handle);
        self.worker.join().map_err(|_| Error::Crashed)?
    }
}
impl Handle {
    pub fn call(&self, message: Message) -> Result<Message, Error> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .try_send(Call::Request { message, reply })
            .map_err(|_| Error::Crashed)?;
        receiver
            .recv_timeout(Duration::from_secs(6))
            .map_err(|_| Error::TimedOut)?
    }

    /// Transfer a scope whose native members the app has already reaped.
    /// The decode reservation never waits for this RPC.
    pub fn release_scope(&self, scope: u32) {
        self.pending_stops.fetch_add(1, Ordering::AcqRel);
        if let Err(error) = self.sender.try_send(Call::ReleaseScope(scope)) {
            eprintln!("MS2_SCOPE_STOP scope={scope} enqueue={error}");
            // The old supervisor cannot be used for another scope if it
            // could not take ownership of this stop.
            let _ = self.pin.kill();
        }
    }
}

fn serve(
    socket: &mut UnixStream,
    generation: u64,
    receiver: &mpsc::Receiver<Call>,
    pending_stops: &AtomicUsize,
) -> Result<(), Error> {
    let mut request = 1_u32;
    let mut transact = |message| {
        request = request.checked_add(1).ok_or(Error::ResourceLimit)?;
        rpc(socket, request, generation, message)
    };
    while let Ok(call) = receiver.recv() {
        match call {
            Call::ReleaseScope(scope) => {
                stop_scope(&mut transact, scope)?;
                pending_stops.fetch_sub(1, Ordering::AcqRel);
            }
            Call::Request { message, reply } => {
                let result = transact(message);
                let failure = result.as_ref().err().copied();
                // A caller timeout can race a successful scope creation. The
                // controller owns any scope whose reply could not be delivered.
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
                if reply.send(result).is_err()
                    && let Some(scope) = abandoned
                {
                    stop_scope(&mut transact, scope)?;
                }
                if let Some(error) = failure {
                    return Err(error);
                }
            }
        }
    }
    Ok(())
}

fn stop_scope(
    transact: &mut impl FnMut(Message) -> Result<Message, Error>,
    scope: u32,
) -> Result<(), Error> {
    let result = match transact(Message::StopScope { scope }) {
        Ok(Message::ScopeStopped { scope: actual }) if actual == scope => Ok(()),
        Ok(Message::Failed { error }) | Err(error) => Err(error),
        Ok(_) => Err(Error::ProtocolViolation),
    };
    if let Err(error) = result {
        eprintln!("MS2_SCOPE_STOP scope={scope} outcome={error:?}");
    }
    result
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
    let frame = read_frame(socket).map_err(|error| match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Error::TimedOut,
        io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset => Error::Crashed,
        _ => Error::ProtocolViolation,
    })?;
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
