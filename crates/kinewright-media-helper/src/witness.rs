//! Deterministic barriers, selectable only by the separate test executable.
use kinewright_job_sys::media_boundary::MediaFile;
use std::{io, os::fd::OwnedFd, sync::OnceLock};

/// The escape attack and its target, fixed once by the stage argument.
static ESCAPE: OnceLock<(String, String)> = OnceLock::new();

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Witness {
    #[default]
    Off,
    BeforeReady,
    Decode,
    BeforeArm,
    BeforeShimArm,
    RssBurst,
    /// Contained and released, the shim never forks and never replies.
    Silent,
    /// A deliberately malicious helper: under the complete boundary it tries
    /// one escape at the first decoder packet. A seccomp denial kills it
    /// (`Crashed`); an errno denial replies `UnsupportedMedia`; an escape
    /// replies `ProtocolViolation` before it closes anything. `control`
    /// attempts nothing.
    Escape,
}
impl Witness {
    pub fn parse(stage: &str) -> io::Result<Self> {
        match stage {
            "before-ready" => Ok(Self::BeforeReady),
            "decode" => Ok(Self::Decode),
            "before-arm" => Ok(Self::BeforeArm),
            "before-shim-arm" => Ok(Self::BeforeShimArm),
            "rss-burst" => Ok(Self::RssBurst),
            "silent" => Ok(Self::Silent),
            stage if stage.starts_with("escape=") => {
                let (attack, target) = stage["escape=".len()..]
                    .split_once('=')
                    .unwrap_or((&stage["escape=".len()..], ""));
                ESCAPE
                    .set((attack.into(), target.into()))
                    .map_err(|_| io::Error::other("escape stage"))?;
                Ok(Self::Escape)
            }
            _ => Err(io::Error::other("unknown witness stage")),
        }
    }
    pub fn before_shim_arm(self, app: u32, spawner: Option<&OwnedFd>) -> io::Result<()> {
        if self == Self::BeforeShimArm {
            let parent;
            let pin = if let Some(spawner) = spawner {
                spawner
            } else {
                parent = kinewright_job_sys::media_boundary::pidfd(app, false)?;
                &parent
            };
            wait_for_loss(pin)?;
        }
        Ok(())
    }
    pub fn before_arm(self, parent: &OwnedFd) -> io::Result<()> {
        if self == Self::BeforeArm {
            // Only parent loss releases the pre-arm barrier; the production
            // arm-and-poll check, rather than this barrier, decides whether to exit.
            wait_for_loss(parent)?;
        }
        Ok(())
    }
    pub fn silent(self) {
        if self == Self::Silent {
            park();
        }
    }
    pub fn before_ready(self) {
        if self == Self::BeforeReady {
            park();
        }
    }
    pub fn decoder_checkpoint(self, response: &mut MediaFile, request: u32, generation: u64) {
        if self == Self::Escape {
            escape(response, request, generation);
            return;
        }
        if matches!(self, Self::Decode | Self::RssBurst) {
            use kinewright_media_ipc::{
                Color, Frame, Kind, Message, ProbeResult, Stream, StreamKind, write_frame,
            };
            let color = kinewright_core::ColorDescription::unknown();
            // A structurally valid but incomplete reply marks a reached native
            // decoder packet call. The decoder/DPB stays live on this stack;
            // Complete is never sent and this metadata must never be published.
            let marker = ProbeResult {
                streams: vec![Stream {
                    index: 0,
                    kind: StreamKind::Video,
                    time_base: [1, 25],
                    timestamp_origin: 0,
                    duration: 1,
                    average_frame_rate: [25, 1],
                }],
                duration_microseconds: 40_000,
                video_stream: Some(0),
                audio_stream: None,
                duration_frames: 1,
                fps: [25, 1],
                kind: Kind::Image,
                resolution: Some([128, 72]),
                color: Color {
                    primaries: color.primaries,
                    transfer: color.transfer,
                    matrix: color.matrix,
                    range: color.range,
                    white_point: color.white_point,
                    bit_depth: color.bit_depth,
                    confidence_basis_points: color.confidence_basis_points,
                    provenance: color.provenance,
                },
            };
            if write_frame(
                response,
                &Frame {
                    request,
                    generation,
                    message: Message::ProbeResult {
                        result: Box::new(marker),
                    },
                },
            )
            .is_err()
            {
                kinewright_job_sys::media_boundary::exit_helper(125);
            }
            if self == Self::RssBurst {
                let resident = vec![0xa5_u8; 384 * 1024 * 1024];
                std::hint::black_box(&resident);
                park();
            }
            park();
        }
    }
}
/// What a successful attack obtained. It stays alive until the helper is
/// killed, so no descriptor teardown (a `close` the filter forbids) can turn
/// an escape into a kill before the reply is written (F3).
enum Obtained {
    File(std::fs::File),
    Socket(OwnedFd),
    Child(std::process::Child),
    Signal,
}

/// Attempt the configured escape and reply as soon as the attack's own
/// syscall returns: `ProtocolViolation` if it obtained a descriptor, a child
/// or a delivered signal, `UnsupportedMedia` if it was denied with an errno.
/// A seccomp kill leaves no reply. Return only for `control`.
fn escape(response: &mut MediaFile, request: u32, generation: u64) {
    use kinewright_media_ipc::{Frame, MediaSandboxError, Message, write_frame};
    use std::io::{Read, Write};
    let Some((attack, target)) = ESCAPE.get() else {
        kinewright_job_sys::media_boundary::exit_helper(125);
    };
    let mut attempt: io::Result<Obtained> = match attack.as_str() {
        "control" => return,
        "read" => std::fs::File::open(target).map(Obtained::File),
        "write" => std::fs::OpenOptions::new()
            .append(true)
            .open(target)
            .map(Obtained::File),
        "socket" => {
            use rustix::net::{AddressFamily, SocketType, socket};
            let (family, kind) = match target.as_str() {
                "inet" => (AddressFamily::INET, SocketType::DGRAM),
                "unix" => (AddressFamily::UNIX, SocketType::DGRAM),
                "netlink" => (AddressFamily::NETLINK, SocketType::RAW),
                "vsock" => (AddressFamily::VSOCK, SocketType::STREAM),
                _ => kinewright_job_sys::media_boundary::exit_helper(125),
            };
            socket(family, kind, None)
                .map(Obtained::Socket)
                .map_err(io::Error::from)
        }
        // Inherited stdio: std opens nothing and needs no error pipe, so the
        // first attack syscall is the process creation itself.
        "spawn" => std::process::Command::new(target)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .map(Obtained::Child),
        "signal" => {
            let Some(pid) = target.parse().ok().and_then(rustix::process::Pid::from_raw) else {
                kinewright_job_sys::media_boundary::exit_helper(125);
            };
            rustix::process::test_kill_process(pid)
                .map(|()| Obtained::Signal)
                .map_err(io::Error::from)
        }
        _ => kinewright_job_sys::media_boundary::exit_helper(125),
    };
    let error = if attempt.is_ok() {
        MediaSandboxError::ProtocolViolation
    } else {
        MediaSandboxError::UnsupportedMedia
    };
    if write_frame(
        response,
        &Frame {
            request,
            generation,
            message: Message::Failed { error },
        },
    )
    .is_err()
    {
        kinewright_job_sys::media_boundary::exit_helper(125);
    }
    // Only after the reply: use what was obtained, so the canary and source
    // checks also see a real read or write. A later denial can only kill a
    // helper that has already answered. Nothing is closed: park never
    // returns, so `attempt` is never dropped, and the host kills the helper.
    if let Ok(obtained) = &mut attempt {
        match obtained {
            Obtained::File(file) if attack == "read" => {
                let _ = file.read(&mut [0; 1]);
            }
            Obtained::File(file) => {
                let _ = file.write_all(b"escaped");
            }
            Obtained::Socket(socket) => {
                let _ = rustix::io::write(&*socket, b"escaped");
            }
            Obtained::Child(child) => {
                let _ = child.try_wait();
            }
            Obtained::Signal => {}
        }
    }
    park();
}

fn park() -> ! {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn wait_for_loss(parent: &OwnedFd) -> io::Result<()> {
    use std::os::fd::AsFd;
    loop {
        match kinewright_job_sys::media_boundary::wait_readable(
            parent.as_fd(),
            std::time::Duration::from_millis(100),
        ) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error),
        }
    }
}
