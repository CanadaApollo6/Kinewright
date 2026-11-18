use crate::{Frame, Kind, MediaSandboxError, Message, ProbeResult, StreamKind};
use std::time::{Duration, Instant};

const SEVEN_DAYS_US: i64 = 604_800_000_000;

/// Only boundary-checked metadata can be consumed by the host.
#[derive(Debug)]
pub struct ValidatedProbe(ProbeResult);
impl ValidatedProbe {
    #[must_use]
    pub fn metadata(&self) -> &ProbeResult {
        &self.0
    }
    #[must_use]
    pub fn into_metadata(self) -> ProbeResult {
        self.0
    }
    pub fn new(result: ProbeResult) -> Result<Self, MediaSandboxError> {
        let bad = MediaSandboxError::ProtocolViolation;
        if result.streams.is_empty() || result.streams.len() > 16 {
            return Err(bad);
        }
        for (index, stream) in result.streams.iter().enumerate() {
            if usize::try_from(stream.index).ok() != Some(index)
                || stream.time_base[0] <= 0
                || stream.time_base[1] <= 0
                || i128::from(stream.timestamp_origin).abs()
                    * i128::from(stream.time_base[0])
                    * 1_000_000
                    > i128::from(SEVEN_DAYS_US) * i128::from(stream.time_base[1])
                || stream.duration < 0
                || i128::from(stream.duration) * i128::from(stream.time_base[0]) * 1_000_000
                    > i128::from(SEVEN_DAYS_US) * i128::from(stream.time_base[1])
                || stream
                    .timestamp_origin
                    .checked_add(stream.duration)
                    .is_none()
            {
                return Err(bad);
            }
            let rate = stream.average_frame_rate;
            let valid = match (stream.kind, result.kind, rate) {
                (StreamKind::Audio, _, rate) => rate == [0, 1],
                (StreamKind::Video, Kind::Image, [0, 1]) => true,
                (StreamKind::Video, _, [n, d]) => {
                    n > 0 && d > 0 && valid_fps(n.unsigned_abs(), d.unsigned_abs())
                }
            };
            if !valid {
                return Err(bad);
            }
        }
        let selected = |id: Option<u32>, kind| {
            id.is_some_and(|id| {
                usize::try_from(id)
                    .ok()
                    .and_then(|id| result.streams.get(id))
                    .is_some_and(|s| s.kind == kind)
            })
        };
        let video = selected(result.video_stream, StreamKind::Video);
        let audio = selected(result.audio_stream, StreamKind::Audio);
        if video != result.video_stream.is_some()
            || audio != result.audio_stream.is_some()
            || !matches!(
                (result.kind, video, audio),
                (Kind::Video | Kind::Image, true, false)
                    | (Kind::Audio, false, true)
                    | (Kind::AudioVideo, true, true)
            )
        {
            return Err(bad);
        }
        let [n, d] = result.fps;
        if !valid_fps(n, d)
            || result.duration_frames <= 0
            || i128::from(result.duration_frames) * i128::from(d) > 604_800 * i128::from(n)
            || result.color.confidence_basis_points > 10_000
            || matches!(
                result.color.primaries,
                kinewright_core::ColorPrimaries::Other(ref tag) if !["smpte428", "ebu3213"].contains(&tag.as_str())
            )
            || matches!(
                result.color.transfer,
                kinewright_core::ColorTransfer::Other(ref tag) if !["smpte240m", "log_sqrt", "iec61966_2_4", "bt1361_ecg", "bt2020_10", "bt2020_12", "smpte428"].contains(&tag.as_str())
            )
            || matches!(result.color.matrix, kinewright_core::ColorMatrix::Other(ref tag) if !["bt470bg", "fcc", "smpte2085", "ipt_c2", "ycgco_re", "ycgco_ro"].contains(&tag.as_str()))
            || matches!(result.color.range, kinewright_core::ColorRange::Other(_))
            || matches!(
                result.color.white_point,
                kinewright_core::ColorWhitePoint::Other(_)
            )
            || matches!(
                result.color.provenance,
                kinewright_core::ColorProvenance::Other(_)
            )
            || matches!(
                result.color.bit_depth,
                kinewright_core::ColorBitDepth::Other(_)
            )
            || matches!(result.color.bit_depth, kinewright_core::ColorBitDepth::Integer(n) if !(1..=32).contains(&n))
        {
            return Err(bad);
        }
        if !(1..=SEVEN_DAYS_US).contains(&result.duration_microseconds)
            || (result.kind == Kind::Image && result.duration_frames != 1)
        {
            return Err(bad);
        }
        match (video, result.resolution) {
            (true, Some([w, h]))
                if (1..=8192).contains(&w)
                    && (1..=8192).contains(&h)
                    && u64::from(w) * u64::from(h) * 8 <= 128 * 1024 * 1024 => {}
            (false, None) => {}
            _ => return Err(bad),
        }
        Ok(Self(result))
    }
}
fn valid_fps(n: u32, d: u32) -> bool {
    n > 0 && d > 0 && u64::from(n) <= u64::from(d) * 4096 && gcd(n, d) == 1
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[derive(Clone, Copy)]
enum State {
    Ready,
    Result,
    Complete,
    Closed,
}

/// Per-source one-shot probe state, including traffic charged before parsing.
/// Launch/deadline authority remains with the host; Ready grants none.
pub struct ProbeSession {
    request: u32,
    generation: u64,
    selected: [Option<u32>; 2],
    state: State,
    epoch: Instant,
    frames: u32,
    bytes: usize,
}
impl ProbeSession {
    #[must_use]
    pub fn new(request: u32, generation: u64, selected: [Option<u32>; 2]) -> Self {
        Self {
            request,
            generation,
            selected,
            state: State::Ready,
            epoch: Instant::now(),
            frames: 0,
            bytes: 0,
        }
    }
    pub fn read(
        &mut self,
        reader: &mut impl std::io::Read,
        now: Instant,
    ) -> std::io::Result<Frame> {
        crate::read_charged(reader, |bytes| {
            self.charge(bytes, now).map_err(|_| crate::invalid())
        })
    }
    pub fn charge(&mut self, bytes: usize, now: Instant) -> Result<(), MediaSandboxError> {
        if now.saturating_duration_since(self.epoch) >= Duration::from_secs(1) {
            self.epoch = now;
            self.frames = 0;
            self.bytes = 0;
        }
        self.frames = self.frames.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        if self.frames > 256 || self.bytes > 16 * 1024 * 1024 {
            self.state = State::Closed;
            return Err(MediaSandboxError::ProtocolViolation);
        }
        Ok(())
    }
    pub fn accept(&mut self, frame: Frame) -> Result<Option<ValidatedProbe>, MediaSandboxError> {
        let bad = MediaSandboxError::ProtocolViolation;
        if frame.request != self.request
            || frame.generation != self.generation
            || self.request == 0
            || self.generation == 0
        {
            self.state = State::Closed;
            return Err(bad);
        }
        let state = std::mem::replace(&mut self.state, State::Closed);
        match (state, frame.message) {
            (State::Ready, Message::Ready {}) => {
                self.state = State::Result;
                Ok(None)
            }
            (State::Result, Message::ProbeResult { result }) => {
                let result = ValidatedProbe::new(*result)?;
                if self.selected != [None, None]
                    && [
                        result.metadata().video_stream,
                        result.metadata().audio_stream,
                    ] != self.selected
                {
                    return Err(bad);
                }
                self.state = State::Complete;
                Ok(Some(result))
            }
            (State::Complete, Message::Complete {}) => Ok(None),
            (State::Ready | State::Result, Message::Failed { error }) => Err(error),
            _ => Err(bad),
        }
    }
}
