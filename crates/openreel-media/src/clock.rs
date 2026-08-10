use openreel_core::{Rational, TimeCode};

/// Convert an audio-device sample-frame position to a project frame.
///
/// All math stays integral; the result is floored so video never runs ahead of audio.
#[must_use]
pub fn samples_to_frame(samples: u64, sample_rate: u32, fps: Rational) -> TimeCode {
    if sample_rate == 0 {
        return TimeCode::ZERO;
    }
    let numerator = u128::from(samples).saturating_mul(u128::from(fps.numerator()));
    let denominator = u128::from(sample_rate).saturating_mul(u128::from(fps.denominator()));
    let frames = numerator / denominator;
    TimeCode(i64::try_from(frames).unwrap_or(i64::MAX))
}

/// Convert a project-frame boundary to an audio-device sample-frame boundary.
#[must_use]
pub fn frame_to_samples(frame: TimeCode, sample_rate: u32, fps: Rational) -> u64 {
    if frame.0 <= 0 || sample_rate == 0 {
        return 0;
    }
    let numerator = u128::try_from(frame.0)
        .unwrap_or_default()
        .saturating_mul(u128::from(sample_rate))
        .saturating_mul(u128::from(fps.denominator()));
    let samples = numerator / u128::from(fps.numerator());
    u64::try_from(samples).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_clock_maps_ntsc_without_float_drift() {
        let fps = Rational::new(30_000, 1_001).unwrap();
        assert_eq!(frame_to_samples(TimeCode(30_000), 48_000, fps), 48_048_000);
        assert_eq!(samples_to_frame(48_048_000, 48_000, fps), TimeCode(30_000));
    }

    #[test]
    fn video_position_is_floored_to_avoid_leading_audio() {
        let fps = Rational::new(30, 1).unwrap();
        assert_eq!(samples_to_frame(1_599, 48_000, fps), TimeCode(0));
        assert_eq!(samples_to_frame(1_600, 48_000, fps), TimeCode(1));
    }
}
