//! Streaming sample arithmetic for the dialogue sound profile.
//!
//! Frames use an integer clock and half-open 10-ms hops. Float arithmetic stays
//! here: channels accumulate squares in f64, and levels round to centidBFS.
//! Platform `log10` can differ by one hundredth on a rounding tie. Profiles are
//! local cache values; no contract compares them across machines.
use super::{DEFAULT_HOP_MICROS, LEVEL_FLOOR, SoundProfile, SourceRange};
use std::num::NonZeroU32;
use thiserror::Error;

/// Stable, small refusal values can be returned again after a failed feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MeasurementError {
    #[error("sample rate must be nonzero")]
    Rate,
    #[error("channel count must be nonzero")]
    Channels,
    #[error("recording contains no complete sample frames")]
    Empty,
    #[error("interleaved samples end inside frame {frame}")]
    PartialFrame { frame: u64 },
    #[error("non-finite sample in frame {frame}")]
    NonFinite { frame: u64 },
    #[error("recording exceeds the profile clock or index range")]
    Capacity,
    #[error("voice detection is unavailable; a sound profile needs explicit voice information")]
    VoiceUnavailable,
    #[error("voice interval {index} is empty, unordered, overlapping or outside the recording")]
    VoiceInterval { index: usize },
    #[error("level measurements do not form a complete sound profile")]
    Profile,
}

/// Holds per-channel energy for one hop and the two output level arrays.
/// It never retains PCM. Chunk boundaries may fall inside a sample frame.
pub struct ProfileBuilder {
    rate: NonZeroU32,
    energy: Vec<f64>,
    channel: usize,
    frames: u64,
    hop_frames: u32,
    peak: f64,
    peaks: Vec<i16>,
    rms: Vec<i16>,
    failure: Option<MeasurementError>,
}

fn frame_hop(frame: u64, rate: NonZeroU32) -> Result<usize, MeasurementError> {
    usize::try_from(
        u128::from(frame) * 1_000_000 / (u128::from(rate.get()) * u128::from(DEFAULT_HOP_MICROS)),
    )
    .map_err(|_| MeasurementError::Capacity)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "rounded finite centidBFS is saturated to the i16 level range"
)]
fn level(amplitude: f64) -> i16 {
    if amplitude == 0.0 {
        LEVEL_FLOOR
    } else {
        (2_000.0 * amplitude.log10())
            .round()
            .clamp(f64::from(LEVEL_FLOOR), 0.0) as i16
    }
}

impl ProfileBuilder {
    /// # Errors
    /// Refuses a zero sample rate or channel count.
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, MeasurementError> {
        let rate = NonZeroU32::new(sample_rate).ok_or(MeasurementError::Rate)?;
        if channels == 0 {
            return Err(MeasurementError::Channels);
        }
        Ok(Self {
            rate,
            energy: vec![0.0; usize::from(channels)],
            channel: 0,
            frames: 0,
            hop_frames: 0,
            peak: 0.0,
            peaks: Vec::new(),
            rms: Vec::new(),
            failure: None,
        })
    }

    /// Feed arbitrary chunks in channel order. An error poisons this builder:
    /// later feeds and finish return that refusal, never a partial quiet profile.
    /// # Errors
    /// Names the frame containing a non-finite sample, or refuses clock overflow.
    pub fn push(&mut self, samples: &[f32]) -> Result<(), MeasurementError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = self.feed(samples);
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }

    fn feed(&mut self, samples: &[f32]) -> Result<(), MeasurementError> {
        for &sample in samples {
            if !sample.is_finite() {
                return Err(MeasurementError::NonFinite { frame: self.frames });
            }
            if self.channel == 0 {
                let hop = frame_hop(self.frames, self.rate)?;
                while self.peaks.len() < hop {
                    self.flush();
                }
            }
            let sample = f64::from(sample);
            self.peak = self.peak.max(sample.abs());
            self.energy[self.channel] += sample * sample;
            self.channel += 1;
            if self.channel == self.energy.len() {
                self.channel = 0;
                self.frames = self
                    .frames
                    .checked_add(1)
                    .ok_or(MeasurementError::Capacity)?;
                self.hop_frames += 1;
            }
        }
        Ok(())
    }

    fn flush(&mut self) {
        let rms = if self.hop_frames == 0 {
            0.0
        } else {
            self.energy
                .iter()
                .map(|sum| (sum / f64::from(self.hop_frames)).sqrt())
                .fold(0.0, f64::max)
        };
        self.peaks.push(level(self.peak));
        // Floating summation must not make RMS exceed the observed peak.
        self.rms.push(level(rms.min(self.peak)));
        self.energy.fill(0.0);
        self.hop_frames = 0;
        self.peak = 0.0;
    }

    /// `Some(&[])` means the detector measured no voice. `None` means voice
    /// information is unavailable and refuses; it never means no speech.
    /// Intervals are half-open source microseconds, ordered and non-overlapping.
    /// # Errors
    /// Refuses a failed/empty/incomplete stream, missing or invalid voice data,
    /// or a duration that cannot be represented by a sound profile.
    pub fn finish(
        mut self,
        voice_intervals: Option<&[SourceRange]>,
    ) -> Result<SoundProfile, MeasurementError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.channel != 0 {
            return Err(MeasurementError::PartialFrame { frame: self.frames });
        }
        if self.frames == 0 {
            return Err(MeasurementError::Empty);
        }
        let intervals = voice_intervals.ok_or(MeasurementError::VoiceUnavailable)?;
        let duration = u64::try_from(
            (u128::from(self.frames) * 1_000_000).div_ceil(u128::from(self.rate.get())),
        )
        .map_err(|_| MeasurementError::Capacity)?;
        let count = usize::try_from(duration.div_ceil(u64::from(DEFAULT_HOP_MICROS)))
            .map_err(|_| MeasurementError::Capacity)?;
        while self.peaks.len() < count {
            self.flush();
        }
        let mut voice = vec![false; count];
        let hop_us = u64::from(DEFAULT_HOP_MICROS);
        let mut previous_end = 0;
        for (index, interval) in intervals.iter().enumerate() {
            if interval.start_micros >= interval.end_micros
                || interval.start_micros < previous_end
                || interval.end_micros > duration
            {
                return Err(MeasurementError::VoiceInterval { index });
            }
            previous_end = interval.end_micros;
            let first = usize::try_from(interval.start_micros / hop_us)
                .map_err(|_| MeasurementError::Capacity)?;
            let end = usize::try_from(interval.end_micros.div_ceil(hop_us))
                .map_err(|_| MeasurementError::Capacity)?;
            voice[first..end].fill(true);
        }
        SoundProfile::from_raw(DEFAULT_HOP_MICROS, duration, self.peaks, self.rms, voice)
            .map_err(|_| MeasurementError::Profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(rate: u32, channels: u16, samples: &[f32]) -> SoundProfile {
        let mut b = ProfileBuilder::new(rate, channels).unwrap();
        b.push(samples).unwrap();
        b.finish(Some(&[])).unwrap()
    }

    #[test]
    fn exact_silence_full_scale_half_scale_and_saturation() {
        for (sample, expected) in [
            (0.0, LEVEL_FLOOR),
            (1.0, 0),
            (-1.0, 0),
            (0.5, -602),
            (2.0, 0),
            (1e-9, LEVEL_FLOOR),
        ] {
            let p = measured(48_000, 1, &[sample; 480]);
            let hop = p.hop(0).unwrap();
            assert_eq!(hop.peak_dbfs_hundredths, expected);
            assert_eq!(hop.rms_dbfs_hundredths, expected);
        }
    }

    #[test]
    fn sine_rms_and_peak_use_amplitude_not_power_decibels() {
        let samples: Vec<_> = (0..480_u16)
            .map(|i| (std::f32::consts::TAU * f32::from(i) / 48.0).sin())
            .collect();
        let p = measured(48_000, 1, &samples);
        assert_eq!(p.hop(0).unwrap().peak_dbfs_hundredths, 0);
        assert!((-302..=-300).contains(&p.hop(0).unwrap().rms_dbfs_hundredths));
    }

    #[test]
    fn stereo_reports_the_loud_channel_not_the_channel_mean() {
        let p = measured(48_000, 2, &[0.0, 1.0].repeat(480));
        assert_eq!(p.hop(0).unwrap().peak_dbfs_hundredths, 0);
        assert_eq!(
            p.hop(0).unwrap().rms_dbfs_hundredths,
            0,
            "MEASURE_CHANNEL_MAX"
        );
    }

    #[test]
    fn chunk_boundaries_including_partial_frames_do_not_change_the_profile() {
        let samples: Vec<_> = (0..1_451_u16)
            .flat_map(|i| [f32::from(i % 17) / 17.0, -f32::from(i % 9) / 9.0])
            .collect();
        let one = measured(22_050, 2, &samples);
        for chunk in [1, 2, 3, 7, 221, 443] {
            let mut builder = ProfileBuilder::new(22_050, 2).unwrap();
            for part in samples.chunks(chunk) {
                builder.push(part).unwrap();
            }
            assert_eq!(one, builder.finish(Some(&[])).unwrap());
        }
    }

    #[test]
    fn integer_hop_clock_is_exact_beyond_float_precision() {
        // 9,007,199,254,741,147 * 100 / 44,100 has quotient
        // 20,424,488,105,988 and remainder 43,900. f64 rounds it up.
        assert_eq!(
            frame_hop(9_007_199_254_741_147, NonZeroU32::new(44_100).unwrap()).unwrap(),
            20_424_488_105_988,
            "MEASURE_INTEGER_HOP_CLOCK"
        );
    }

    #[test]
    fn first_three_hops_and_final_short_hop_have_hand_counted_frames() {
        // Boundary starts imply 441/441/441, 480/480/480, 221/220/221,
        // and 160/160/160 frames. The final hop has one frame in each case.
        for (rate, starts, frames, duration, rms) in [
            (
                44_100,
                [0, 441, 882, 1_323],
                1_324,
                30_023,
                [-2343, -2548, -2618, -1806],
            ),
            (
                48_000,
                [0, 480, 960, 1_440],
                1_441,
                30_021,
                [-2380, -2584, -2655, -1806],
            ),
            (
                22_050,
                [0, 221, 441, 662],
                663,
                30_069,
                [-2043, -2246, -2318, -1806],
            ),
            (
                16_000,
                [0, 160, 320, 480],
                481,
                30_063,
                [-1903, -2107, -2178, -1806],
            ),
        ] {
            let mut samples = vec![0.0; frames];
            for (i, start) in starts.into_iter().enumerate() {
                samples[start] = [1.0, 0.5, 0.25, 0.125][i];
                if start > 0 {
                    samples[start - 1] = 1.0;
                }
            }
            let profile = measured(rate, 1, &samples);
            assert_eq!(profile.duration_micros(), duration);
            assert_eq!(profile.len(), 4);
            assert_eq!(
                profile
                    .hops()
                    .map(|h| h.peak_dbfs_hundredths)
                    .collect::<Vec<_>>(),
                [0, 0, 0, -1806]
            );
            assert_eq!(
                profile
                    .hops()
                    .map(|h| h.rms_dbfs_hundredths)
                    .collect::<Vec<_>>(),
                rms
            );
        }
    }

    #[test]
    fn duration_covers_the_last_sample_frame_and_empty_tail_hops() {
        let p = measured(44_100, 1, &[0.5]);
        assert_eq!(p.duration_micros(), 23, "MEASURE_DURATION_CEIL");
        let p = measured(1, 1, &[1.0, 0.5]);
        assert_eq!(p.duration_micros(), 2_000_000);
        assert_eq!(p.len(), 200);
        assert_eq!(p.hop(0).unwrap().peak_dbfs_hundredths, 0);
        assert_eq!(p.hop(100).unwrap().peak_dbfs_hundredths, -602);
        assert_eq!(p.hop(199).unwrap().peak_dbfs_hundredths, LEVEL_FLOOR);
    }

    #[test]
    fn malformed_samples_and_configuration_refuse_with_the_frame() {
        assert!(matches!(
            ProfileBuilder::new(0, 1),
            Err(MeasurementError::Rate)
        ));
        assert!(matches!(
            ProfileBuilder::new(1, 0),
            Err(MeasurementError::Channels)
        ));
        assert_eq!(
            ProfileBuilder::new(48_000, 1).unwrap().finish(Some(&[])),
            Err(MeasurementError::Empty)
        );
        let mut partial = ProfileBuilder::new(48_000, 2).unwrap();
        partial.push(&[0.5]).unwrap();
        assert_eq!(
            partial.finish(Some(&[])),
            Err(MeasurementError::PartialFrame { frame: 0 })
        );
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut b = ProfileBuilder::new(48_000, 2).unwrap();
            b.push(&[0.0, 1.0, 0.5]).unwrap();
            let expected = Err(MeasurementError::NonFinite { frame: 1 });
            assert_eq!(b.push(&[bad]), expected);
            assert_eq!(b.push(&[0.0]), expected);
            assert_eq!(
                b.finish(Some(&[])),
                Err(MeasurementError::NonFinite { frame: 1 })
            );
        }
    }

    #[test]
    fn voice_overlap_is_half_open_and_unavailable_is_not_no_voice() {
        let samples = [0.0; 1_440];
        let build = || {
            let mut b = ProfileBuilder::new(48_000, 1).unwrap();
            b.push(&samples).unwrap();
            b
        };
        assert_eq!(
            build().finish(None),
            Err(MeasurementError::VoiceUnavailable)
        );
        assert!(build().finish(Some(&[])).unwrap().hops().all(|h| !h.voice));
        let p = build()
            .finish(Some(&[SourceRange {
                start_micros: 0,
                end_micros: 10_000,
            }]))
            .unwrap();
        assert_eq!(
            p.hops().map(|h| h.voice).collect::<Vec<_>>(),
            [true, false, false],
            "MEASURE_VOICE_HALF_OPEN_END"
        );
        let p = build()
            .finish(Some(&[SourceRange {
                start_micros: 10_000,
                end_micros: 20_000,
            }]))
            .unwrap();
        assert_eq!(
            p.hops().map(|h| h.voice).collect::<Vec<_>>(),
            [false, true, false]
        );
        let p = build()
            .finish(Some(&[SourceRange {
                start_micros: 9_999,
                end_micros: 10_001,
            }]))
            .unwrap();
        assert_eq!(
            p.hops().map(|h| h.voice).collect::<Vec<_>>(),
            [true, true, false]
        );
        let p = build()
            .finish(Some(&[
                SourceRange {
                    start_micros: 0,
                    end_micros: 10_000,
                },
                SourceRange {
                    start_micros: 10_000,
                    end_micros: 30_000,
                },
            ]))
            .unwrap();
        assert!(p.hops().all(|h| h.voice));
    }

    #[test]
    fn invalid_voice_intervals_refuse_instead_of_losing_speech() {
        for ranges in [
            vec![SourceRange {
                start_micros: 1,
                end_micros: 1,
            }],
            vec![SourceRange {
                start_micros: 20_000,
                end_micros: 30_001,
            }],
            vec![
                SourceRange {
                    start_micros: 9,
                    end_micros: 20,
                },
                SourceRange {
                    start_micros: 5,
                    end_micros: 8,
                },
            ],
            vec![
                SourceRange {
                    start_micros: 9,
                    end_micros: 20,
                },
                SourceRange {
                    start_micros: 19,
                    end_micros: 21,
                },
            ],
        ] {
            assert!(matches!(
                {
                    let mut b = ProfileBuilder::new(48_000, 1).unwrap();
                    b.push(&[0.0; 1440]).unwrap();
                    b.finish(Some(&ranges))
                },
                Err(MeasurementError::VoiceInterval { .. })
            ));
        }
    }
}
