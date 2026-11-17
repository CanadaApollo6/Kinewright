use crate::bus::{self, Bus};
use kinewright_job_sys::{Cgroup, ProcessHandle, media_scope as native};
use kinewright_media_ipc::{
    Frame, MediaSandboxError as Error, Message, ProbeSession, ScopeUnavailableReason, write_frame,
};
use std::{
    io,
    net::Shutdown,
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

struct Record {
    id: u32,
    unit: String,
    process: ProcessHandle,
    memory: u64,
    group: Option<Cgroup>,
}
struct Service {
    app: u32,
    generation: u64,
    own_start: u64,
    bus: Option<Bus>,
    records: Vec<Record>,
    next: u32,
}
impl Service {
    fn create(&mut self, pid: u32, memory: u64) -> Message {
        if pid == self.app || pid == std::process::id() {
            return Message::Failed {
                error: Error::ProtocolViolation,
            };
        }
        if self.records.len() >= 2 {
            return Message::Failed {
                error: Error::ResourceLimit,
            };
        }
        if !(64 * 1024 * 1024..=2 * 1024 * 1024 * 1024).contains(&memory) {
            return Message::Failed {
                error: Error::ProtocolViolation,
            };
        }
        if self.bus.is_none() {
            return Message::ScopeUnavailable {
                scope: None,
                reason: ScopeUnavailableReason::NoUserManager,
            };
        }
        let Ok(process) = native::child(self.app, pid) else {
            return Message::Failed {
                error: Error::ProtocolViolation,
            };
        };
        if self
            .records
            .iter()
            .any(|record| record.process.pid() == pid)
        {
            return Message::Failed {
                error: Error::ProtocolViolation,
            };
        }
        let Some(id) = self.next.checked_add(1) else {
            return Message::Failed {
                error: Error::ResourceLimit,
            };
        };
        self.next = id;
        self.records.push(Record {
            id,
            unit: format!(
                "kinewright-media-{}-{}-{}-{}-{id}.scope",
                self.app,
                std::process::id(),
                self.own_start,
                self.generation
            ),
            process,
            memory,
            group: None,
        });
        let record = &self.records[self.records.len() - 1];
        let result = self.bus.as_ref().ok_or(()).and_then(|bus| {
            bus.start(&record.unit, &record.process, memory)
                .map_err(|_| ())
        });
        if result.is_err() {
            // The bus may have created the unit even when its reply was lost.
            // Keep the bounded record and give the host its cleanup identity.
            Message::ScopeCreateFailed {
                scope: id,
                error: Error::SandboxUnavailable,
            }
        } else {
            Message::ScopeCreated { scope: id }
        }
    }
    fn verify(&mut self, id: u32) -> Message {
        let Some(index) = self.records.iter().position(|record| record.id == id) else {
            return Message::Failed {
                error: Error::ProtocolViolation,
            };
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let result = loop {
            match self.verify_record(index) {
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                result => break result,
            }
        };
        match result {
            Ok(()) => Message::ScopeVerified {
                scope: id,
                starttime: self.records[index].process.start(),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Message::ScopeUnavailable {
                scope: Some(id),
                reason: ScopeUnavailableReason::ControllersUnavailable,
            },
            Err(error) => {
                eprintln!("MS2_SCOPE_VERIFY {id}: {error}");
                Message::Failed {
                    error: Error::SandboxUnavailable,
                }
            }
        }
    }
    fn verify_record(&mut self, index: usize) -> io::Result<()> {
        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| io::Error::other("user manager unavailable"))?;
        let record = &mut self.records[index];
        native::verify_child(self.app, &record.process)?;
        let path = bus.unit(&record.unit).map_err(io::Error::other)?;
        if !bus.active(&path).map_err(io::Error::other)? {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "scope start pending",
            ));
        }
        if !bus
            .verify_controls(&path, record.memory)
            .map_err(io::Error::other)?
        {
            return Err(io::Error::other(
                "scope not active or configured controls mismatch",
            ));
        }
        let group_path = native::checked_group(&bus.group(&path).map_err(io::Error::other)?)?;
        native::outside(self.app, &group_path)?;
        if native::membership(record.process.pid())? != group_path {
            return Err(io::Error::other("scope membership mismatch"));
        }
        if let Some(retained) = record.group.as_ref() {
            if retained.path() != group_path {
                return Err(io::Error::other("scope cgroup changed"));
            }
        } else {
            record.group = Some(Cgroup::open(&group_path)?);
        }
        let group = record
            .group
            .as_ref()
            .ok_or_else(|| io::Error::other("missing retained cgroup"))?;
        native::verify_controls(group, record.memory)?;
        native::verify_child(self.app, &record.process)
    }
    fn stop_record(&mut self, index: usize) -> io::Result<bool> {
        let record = &mut self.records[index];
        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| io::Error::other("user manager unavailable"))?;
        let stop = bus.stop(&record.unit);
        if record.group.is_none() {
            match bus.unit(&record.unit) {
                Ok(path) => {
                    let path = native::checked_group(&bus.group(&path).map_err(io::Error::other)?)?;
                    native::outside(self.app, &path)?;
                    record.group = Some(Cgroup::open(&path)?);
                }
                Err(error) if bus::absent(&error) => {
                    stop.map_err(io::Error::other)?;
                    return Ok(true);
                }
                Err(error) => return Err(io::Error::other(error)),
            }
        }
        let group = record
            .group
            .as_mut()
            .ok_or_else(|| io::Error::other("missing retained cgroup"))?;
        if !group.empty()? {
            return Ok(false);
        }
        // Native emptiness is the release condition. The generated unit is also
        // stopped/reset; CollectMode makes an empty failed unit collectible.
        stop.map_err(io::Error::other)?;
        bus.reset(&record.unit).map_err(io::Error::other)?;
        Ok(true)
    }
    fn stop(&mut self, id: u32) -> Message {
        let Some(index) = self.records.iter().position(|record| record.id == id) else {
            return Message::Failed {
                error: Error::ProtocolViolation,
            };
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match self.stop_record(index) {
                Ok(true) => {
                    self.records.remove(index);
                    return Message::ScopeStopped { scope: id };
                }
                Err(_) => {
                    return Message::Failed {
                        error: Error::SandboxUnavailable,
                    };
                }
                Ok(false) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(false) => {
                    return Message::Failed {
                        error: Error::TimedOut,
                    };
                }
            }
        }
    }
    fn shutdown(&mut self) {
        // Keep ownership until every native scope is empty, even when the app
        // socket disappears. This loop admits no new work and creates no units.
        while !self.records.is_empty() {
            let mut retry = false;
            for index in (0..self.records.len()).rev() {
                if matches!(self.stop_record(index), Ok(true)) {
                    self.records.remove(index);
                } else {
                    retry = true;
                }
            }
            if retry {
                self.bus = Bus::connect().ok();
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    fn serve(&mut self, socket: &mut UnixStream) -> io::Result<()> {
        let initial = if self.bus.is_some() {
            Message::Ready {}
        } else {
            Message::ScopeUnavailable {
                scope: None,
                reason: ScopeUnavailableReason::NoUserManager,
            }
        };
        write_frame(
            socket,
            &Frame {
                request: 1,
                generation: self.generation,
                message: initial,
            },
        )?;
        let mut last = 1;
        let mut traffic = ProbeSession::new(1, self.generation, [None, None]);
        loop {
            // Charge framing before allocation; lifecycle sequencing is separate
            // from the probe state machine and never accepts media commands.
            let frame = traffic.read(socket, Instant::now())?;
            if frame.generation != self.generation || frame.request <= last {
                return Err(io::Error::other("stale lifecycle request"));
            }
            last = frame.request;
            let message = match frame.message {
                Message::CreateScope { pid, memory_bytes } => self.create(pid, memory_bytes),
                Message::VerifyScope { scope } => self.verify(scope),
                Message::StopScope { scope } => self.stop(scope),
                _ => return Err(io::Error::other("non-lifecycle supervisor command")),
            };
            write_frame(
                socket,
                &Frame {
                    request: frame.request,
                    generation: self.generation,
                    message,
                },
            )?;
        }
    }
}
pub fn run() -> io::Result<()> {
    let (app, generation, mut socket) = crate::bootstrap::input()?;
    let own_start = ProcessHandle::open(std::process::id())?.start();
    let mut service = Service {
        app,
        generation,
        own_start,
        bus: Bus::connect().ok(),
        records: Vec::with_capacity(2),
        next: 0,
    };
    let result = service.serve(&mut socket);
    let _ = socket.shutdown(Shutdown::Both);
    service.shutdown();
    match result {
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(()),
        other => other,
    }
}
