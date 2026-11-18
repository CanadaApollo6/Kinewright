//! Untrusted media and lifecycle traffic. No `FFmpeg` or operating-system authority.
#![allow(clippy::missing_errors_doc)]

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

mod json;
mod validate;
pub use validate::{ProbeSession, ValidatedProbe};

pub const MAX_FRAME: usize = 65_536;
pub const HEADER_BYTES: usize = 40;
// Exact build protocol. Incompatible builds refuse rather than negotiate.
const BUILD: [u8; 16] = *b"KW-MS2-PROBE-002";
const MAGIC: [u8; 4] = *b"KWMP";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum MediaSandboxError {
    SandboxUnavailable,
    Crashed,
    TimedOut,
    ResourceLimit,
    ProtocolViolation,
    UnsupportedMedia,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum StreamKind {
    Video,
    Audio,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    pub index: u32,
    pub kind: StreamKind,
    pub time_base: [i32; 2],
    pub timestamp_origin: i64,
    pub duration: i64,
    /// Reduced FPS, or [0, 1] for audio and still streams without a rate.
    pub average_frame_rate: [i32; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Color {
    pub primaries: kinewright_core::ColorPrimaries,
    pub transfer: kinewright_core::ColorTransfer,
    pub matrix: kinewright_core::ColorMatrix,
    pub range: kinewright_core::ColorRange,
    pub white_point: kinewright_core::ColorWhitePoint,
    pub bit_depth: kinewright_core::ColorBitDepth,
    pub confidence_basis_points: u16,
    pub provenance: kinewright_core::ColorProvenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Kind {
    Video,
    Audio,
    AudioVideo,
    Image,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeResult {
    pub streams: Vec<Stream>,
    pub duration_microseconds: i64,
    pub video_stream: Option<u32>,
    pub audio_stream: Option<u32>,
    pub duration_frames: i64,
    pub fps: [u32; 2],
    pub kind: Kind,
    pub resolution: Option<[u32; 2]>,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ScopeUnavailableReason {
    NoUserManager,
    ControllersUnavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Message {
    ScopeCreateFailed {
        scope: u32,
        error: MediaSandboxError,
    },
    ScopeUnavailable {
        scope: Option<u32>,
        reason: ScopeUnavailableReason,
    },
    Forked {
        pid: u32,
    },
    EnterBoundary {},
    Boundary {
        landlock: bool,
    },
    ConfirmBoundary {},
    Probe {
        video: Option<u32>,
        audio: Option<u32>,
    },
    Cancel {},
    Close {},
    Ready {},
    ProbeResult {
        result: Box<ProbeResult>,
    },
    Complete {},
    Failed {
        error: MediaSandboxError,
    },
    CreateScope {
        pid: u32,
        memory_bytes: u64,
    },
    VerifyScope {
        scope: u32,
    },
    StopScope {
        scope: u32,
    },
    ScopeCreated {
        scope: u32,
    },
    ScopeVerified {
        scope: u32,
        starttime: u64,
    },
    ScopeStopped {
        scope: u32,
    },
}
impl Message {
    fn tag(&self) -> u32 {
        match self {
            Self::ScopeCreateFailed { .. } => 18,
            Self::ScopeUnavailable { .. } => 19,
            Self::Forked { .. } => 14,
            Self::EnterBoundary {} => 15,
            Self::Boundary { .. } => 16,
            Self::ConfirmBoundary {} => 17,
            Self::Probe { .. } => 1,
            Self::Cancel {} => 2,
            Self::Close {} => 3,
            Self::Ready {} => 4,
            Self::ProbeResult { .. } => 5,
            Self::Complete {} => 6,
            Self::Failed { .. } => 7,
            Self::CreateScope { .. } => 8,
            Self::VerifyScope { .. } => 9,
            Self::StopScope { .. } => 10,
            Self::ScopeCreated { .. } => 11,
            Self::ScopeVerified { .. } => 12,
            Self::ScopeStopped { .. } => 13,
        }
    }
}

#[derive(Debug)]
pub struct Frame {
    pub request: u32,
    pub generation: u64,
    pub message: Message,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "media protocol violation")
}

pub fn read_frame(reader: &mut impl Read) -> io::Result<Frame> {
    read_charged(reader, |_| Ok(()))
}
pub(crate) fn read_charged(
    reader: &mut impl Read,
    mut charge: impl FnMut(usize) -> io::Result<()>,
) -> io::Result<Frame> {
    let mut header = [0; HEADER_BYTES];
    reader.read_exact(&mut header)?;
    if header[..4] != MAGIC || header[4..20] != BUILD {
        charge(HEADER_BYTES)?;
        return Err(invalid());
    }
    let tag = u32::from_le_bytes(header[20..24].try_into().map_err(|_| invalid())?);
    let length = u32::from_le_bytes(header[24..28].try_into().map_err(|_| invalid())?) as usize;
    if length > MAX_FRAME - HEADER_BYTES || !(1..=19).contains(&tag) {
        charge(HEADER_BYTES)?;
        return Err(invalid());
    }
    charge(HEADER_BYTES + length)?;
    let request = u32::from_le_bytes(header[28..32].try_into().map_err(|_| invalid())?);
    let generation = u64::from_le_bytes(header[32..40].try_into().map_err(|_| invalid())?);
    if request == 0 || generation == 0 {
        return Err(invalid());
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    json::check(&payload)?;
    let message: Message = serde_json::from_slice(&payload).map_err(|_| invalid())?;
    if message.tag() != tag {
        return Err(invalid());
    }
    Ok(Frame {
        request,
        generation,
        message,
    })
}

pub fn write_frame(writer: &mut impl Write, frame: &Frame) -> io::Result<()> {
    let payload = serde_json::to_vec(&frame.message).map_err(|_| invalid())?;
    if payload.len() > MAX_FRAME - HEADER_BYTES || frame.request == 0 || frame.generation == 0 {
        return Err(invalid());
    }
    let mut header = [0; HEADER_BYTES];
    header[..4].copy_from_slice(&MAGIC);
    header[4..20].copy_from_slice(&BUILD);
    header[20..24].copy_from_slice(&frame.message.tag().to_le_bytes());
    header[24..28].copy_from_slice(
        &u32::try_from(payload.len())
            .map_err(|_| invalid())?
            .to_le_bytes(),
    );
    header[28..32].copy_from_slice(&frame.request.to_le_bytes());
    header[32..].copy_from_slice(&frame.generation.to_le_bytes());
    writer.write_all(&header)?;
    writer.write_all(&payload)?;
    writer.flush()
}

#[cfg(test)]
mod tests;
