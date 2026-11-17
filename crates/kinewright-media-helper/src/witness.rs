//! Deterministic barriers, selectable only by the separate test executable.
use kinewright_job_sys::media_boundary::MediaFile;
use std::{io, os::fd::OwnedFd};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Witness {
    #[default]
    Off,
    BeforeReady,
    Decode,
    BeforeArm,
    BeforeShimArm,
}
impl Witness {
    pub fn parse(stage: &str) -> io::Result<Self> {
        match stage {
            "before-ready" => Ok(Self::BeforeReady),
            "decode" => Ok(Self::Decode),
            "before-arm" => Ok(Self::BeforeArm),
            "before-shim-arm" => Ok(Self::BeforeShimArm),
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
    pub fn before_ready(self) {
        if self == Self::BeforeReady {
            park();
        }
    }
    pub fn decoder_checkpoint(self, response: &mut MediaFile, request: u32, generation: u64) {
        if self == Self::Decode {
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
            park();
        }
    }
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
