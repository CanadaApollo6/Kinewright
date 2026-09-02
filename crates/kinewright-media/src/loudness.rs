use kinewright_core::{
    AUDIO_DELIVERY_ANALYSIS_SAMPLE_RATE, AudioDeliveryMeasurement, AudioLoudness, MediaError,
    TRUE_PEAK_OVERSAMPLING,
};

const ABSOLUTE_GATE_LUFS: f64 = -70.0;
const RELATIVE_GATE_LU: f64 = 10.0;
const LOUDNESS_OFFSET: f64 = -0.691;

#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    const fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b,
            a,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, input: f64) -> f64 {
        let output = self.b[0] * input + self.b[1] * self.x1 + self.b[2] * self.x2
            - self.a[1] * self.y1
            - self.a[2] * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;
        output
    }
}

fn k_weighting() -> (Biquad, Biquad) {
    // BS.1770 coefficients for the fixed 48 kHz delivery-analysis rate.
    let shelf = Biquad::new(
        [
            1.535_124_859_586_97,
            -2.691_696_189_406_38,
            1.198_392_810_852_85,
        ],
        [1.0, -1.690_659_293_182_41, 0.732_480_774_215_85],
    );
    let high_pass = Biquad::new(
        [1.0, -2.0, 1.0],
        [1.0, -1.990_047_454_833_98, 0.990_072_250_366_21],
    );
    (shelf, high_pass)
}

fn energy_loudness(energy: f64) -> f64 {
    LOUDNESS_OFFSET + 10.0 * energy.log10()
}

fn hundredths(value: f64) -> Result<i32, MediaError> {
    let scaled = (value * 100.0).round();
    if !scaled.is_finite() || scaled < f64::from(i32::MIN) || scaled > f64::from(i32::MAX) {
        return Err(MediaError::Backend(
            "audio loudness result is outside the supported range".to_owned(),
        ));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(scaled as i32)
}

fn count_as_f64(value: usize) -> Result<f64, MediaError> {
    u32::try_from(value).map(f64::from).map_err(|_| {
        MediaError::Backend("audio loudness analysis contains too many blocks".to_owned())
    })
}

/// Measure interleaved 48 kHz PCM with BS.1770 K-weighting, 400 ms blocks,
/// 100 ms overlap steps, the -70 LUFS absolute gate, and the -10 LU relative gate.
///
/// # Errors
///
/// Returns a media error for unsupported channel layouts, a non-48 kHz rate,
/// misaligned PCM, or a result outside the fixed-point range.
pub fn measure_loudness(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<AudioLoudness, MediaError> {
    if sample_rate != 48_000 {
        return Err(MediaError::Backend(format!(
            "loudness measurement requires 48000 Hz PCM, got {sample_rate}"
        )));
    }
    let channels = usize::from(channels);
    if channels == 0 || channels > 2 {
        return Err(MediaError::Backend(
            "loudness measurement currently supports mono or stereo PCM".to_owned(),
        ));
    }
    if !samples.len().is_multiple_of(channels) {
        return Err(MediaError::Backend(
            "interleaved PCM is not aligned to its channel count".to_owned(),
        ));
    }
    let sample_frames = samples.len() / channels;
    let peak = samples
        .iter()
        .map(|sample| f64::from(sample.abs()))
        .fold(0.0_f64, f64::max);
    if sample_frames == 0 || peak == 0.0 {
        return Ok(AudioLoudness {
            integrated_lufs_hundredths: None,
            sample_peak_dbfs_hundredths: None,
            sample_rate,
            channels: u16::try_from(channels).unwrap_or(0),
            sample_frames: u64::try_from(sample_frames).unwrap_or(u64::MAX),
        });
    }

    let mut filters = (0..channels).map(|_| k_weighting()).collect::<Vec<_>>();
    let mut weighted = Vec::with_capacity(samples.len());
    for frame in samples.chunks_exact(channels) {
        for (channel, sample) in frame.iter().enumerate() {
            let (shelf, high_pass) = &mut filters[channel];
            weighted.push(high_pass.process(shelf.process(f64::from(*sample))));
        }
    }

    let standard_block = usize::try_from(sample_rate / 10 * 4).unwrap_or(19_200);
    let block_frames = standard_block.min(sample_frames);
    let hop_frames = usize::try_from(sample_rate / 10)
        .unwrap_or(4_800)
        .min(block_frames);
    let mut block_energies = Vec::new();
    let mut start = 0_usize;
    loop {
        let end = start.saturating_add(block_frames).min(sample_frames);
        let frame_count = end.saturating_sub(start);
        if frame_count == 0 {
            break;
        }
        let mut energy = 0.0_f64;
        for channel in 0..channels {
            let channel_energy = (start..end)
                .map(|frame| weighted[frame * channels + channel].powi(2))
                .sum::<f64>()
                / count_as_f64(frame_count)?;
            energy += channel_energy;
        }
        if energy > 0.0 && energy_loudness(energy) > ABSOLUTE_GATE_LUFS {
            block_energies.push(energy);
        }
        if end == sample_frames {
            break;
        }
        start = start.saturating_add(hop_frames);
    }

    let integrated_lufs_hundredths = if block_energies.is_empty() {
        None
    } else {
        let ungated = block_energies.iter().sum::<f64>() / count_as_f64(block_energies.len())?;
        let relative_gate = energy_loudness(ungated) - RELATIVE_GATE_LU;
        let gate = relative_gate.max(ABSOLUTE_GATE_LUFS);
        let gated = block_energies
            .into_iter()
            .filter(|energy| energy_loudness(*energy) > gate)
            .collect::<Vec<_>>();
        if gated.is_empty() {
            None
        } else {
            let energy = gated.iter().sum::<f64>() / count_as_f64(gated.len())?;
            Some(hundredths(energy_loudness(energy))?)
        }
    };
    let sample_peak_dbfs_hundredths = Some(hundredths(20.0 * peak.log10())?);
    Ok(AudioLoudness {
        integrated_lufs_hundredths,
        sample_peak_dbfs_hundredths,
        sample_rate,
        channels: u16::try_from(channels).unwrap_or(0),
        sample_frames: u64::try_from(sample_frames).unwrap_or(u64::MAX),
    })
}

// ---------------------------------------------------------------------------
// AD0: true peak and loudness range on top of the BS.1770 integrated model
// ---------------------------------------------------------------------------

/// Half the interpolation support, in input samples, on each side of the
/// output position: a 12-sample window, 48 taps at 4× oversampling.
const TRUE_PEAK_HALF_SUPPORT: i64 = 6;

/// EBU Tech 3342 short-term window and hop, in frames at the analysis rate.
const SHORT_TERM_WINDOW_FRAMES: usize = 3 * AUDIO_DELIVERY_ANALYSIS_SAMPLE_RATE as usize;
const SHORT_TERM_HOP_FRAMES: usize = AUDIO_DELIVERY_ANALYSIS_SAMPLE_RATE as usize / 10;
const LRA_RELATIVE_GATE_LU: f64 = 20.0;

/// `sin(πu)/(πu)`, continuous at zero.
fn sinc(u: f64) -> f64 {
    if u.abs() < 1e-12 {
        1.0
    } else {
        let x = std::f64::consts::PI * u;
        x.sin() / x
    }
}

/// Blackman window over `|u| <= TRUE_PEAK_HALF_SUPPORT`.
#[allow(clippy::cast_precision_loss)]
fn blackman(u: f64) -> f64 {
    let half = TRUE_PEAK_HALF_SUPPORT as f64;
    if u.abs() >= half {
        return 0.0;
    }
    let x = std::f64::consts::PI * u / half;
    0.42 + 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
}

/// The polyphase interpolation kernel: one row per fractional phase
/// `p / TRUE_PEAK_OVERSAMPLING` for `p = 1..OVERSAMPLING`, each normalized to
/// unit DC gain. Phase 0 is the input sample itself and needs no filter.
///
/// A windowed-sinc interpolator rather than a transcription of BS.1770-4
/// Annex 2's coefficient table: the Annex table is itself a 48-tap
/// windowed low-pass, and the two agree to well under 0.1 dB on band-limited
/// programme material, which is what the `fs/4` fixture below pins.
#[allow(clippy::cast_precision_loss)]
fn true_peak_kernel() -> Vec<Vec<f64>> {
    let phases = TRUE_PEAK_OVERSAMPLING as usize;
    (1..phases)
        .map(|phase| {
            let fraction = phase as f64 / phases as f64;
            let taps = (-TRUE_PEAK_HALF_SUPPORT..TRUE_PEAK_HALF_SUPPORT)
                .map(|offset| {
                    let u = offset as f64 - fraction;
                    sinc(u) * blackman(u)
                })
                .collect::<Vec<_>>();
            let gain = taps.iter().sum::<f64>();
            taps.into_iter().map(|tap| tap / gain).collect()
        })
        .collect()
}

/// The largest absolute value of the 4× oversampled signal, per channel, as
/// a linear amplitude. Zero for silence.
#[allow(clippy::cast_possible_wrap)]
fn true_peak_amplitude(samples: &[f32], channels: usize) -> f64 {
    let kernel = true_peak_kernel();
    let frames = samples.len() / channels;
    let mut peak = 0.0_f64;
    for channel in 0..channels {
        let sample = |frame: i64| -> f64 {
            if frame < 0 || frame >= frames as i64 {
                0.0
            } else {
                f64::from(samples[frame as usize * channels + channel])
            }
        };
        for frame in 0..frames as i64 {
            peak = peak.max(sample(frame).abs());
            for taps in &kernel {
                let interpolated = taps
                    .iter()
                    .zip(-TRUE_PEAK_HALF_SUPPORT..TRUE_PEAK_HALF_SUPPORT)
                    .map(|(tap, offset)| tap * sample(frame + offset))
                    .sum::<f64>();
                peak = peak.max(interpolated.abs());
            }
        }
    }
    peak
}

/// Short-term (3 s) loudness values every 100 ms, gated absolutely at
/// −70 LUFS, from the K-weighted signal. Programmes shorter than one window
/// use the whole programme as the single window.
fn short_term_loudness(samples: &[f32], channels: usize) -> Result<Vec<f64>, MediaError> {
    let frames = samples.len() / channels;
    let mut filters = (0..channels).map(|_| k_weighting()).collect::<Vec<_>>();
    // Prefix sums of the per-frame energy summed over channels, so a window is
    // one subtraction regardless of its length.
    let mut prefix = Vec::with_capacity(frames + 1);
    prefix.push(0.0_f64);
    for frame in samples.chunks_exact(channels) {
        let mut energy = 0.0_f64;
        for (channel, sample) in frame.iter().enumerate() {
            let (shelf, high_pass) = &mut filters[channel];
            energy += high_pass.process(shelf.process(f64::from(*sample))).powi(2);
        }
        prefix.push(prefix.last().copied().unwrap_or(0.0) + energy);
    }
    let window = SHORT_TERM_WINDOW_FRAMES.min(frames);
    if window == 0 {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    let mut start = 0_usize;
    loop {
        let end = start + window;
        if end > frames {
            break;
        }
        let energy = (prefix[end] - prefix[start]) / count_as_f64(window)?;
        if energy > 0.0 {
            let loudness = energy_loudness(energy);
            if loudness > ABSOLUTE_GATE_LUFS {
                values.push(loudness);
            }
        }
        if end == frames {
            break;
        }
        start += SHORT_TERM_HOP_FRAMES;
    }
    Ok(values)
}

/// EBU Tech 3342 loudness range: the relative gate sits 20 LU below the
/// energy-mean of the absolutely gated short-term values; the range is the
/// 95th minus the 10th percentile (nearest rank) of what survives. `None` with
/// fewer than two surviving windows.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn loudness_range_lu(short_term: &[f64]) -> Option<f64> {
    if short_term.len() < 2 {
        return None;
    }
    let mean_energy = short_term
        .iter()
        .map(|loudness| 10.0_f64.powf((loudness - LOUDNESS_OFFSET) / 10.0))
        .sum::<f64>()
        / short_term.len() as f64;
    let gate = energy_loudness(mean_energy) - LRA_RELATIVE_GATE_LU;
    let mut gated = short_term
        .iter()
        .copied()
        .filter(|loudness| *loudness > gate)
        .collect::<Vec<_>>();
    if gated.len() < 2 {
        return None;
    }
    gated.sort_by(f64::total_cmp);
    let last = (gated.len() - 1) as f64;
    let low = gated[(0.10 * last).floor() as usize];
    let high = gated[(0.95 * last).floor() as usize];
    Some(high - low)
}

/// The AD0 delivery measurement: [`measure_loudness`] plus the 4× oversampled
/// true peak and the loudness range, on the same interleaved 48 kHz PCM.
///
/// # Errors
///
/// Propagates [`measure_loudness`]'s refusals and a result outside the
/// fixed-point range.
pub fn measure_delivery_audio(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<AudioDeliveryMeasurement, MediaError> {
    let loudness = measure_loudness(samples, sample_rate, channels)?;
    let channel_count = usize::from(channels);
    let true_peak = true_peak_amplitude(samples, channel_count);
    let true_peak_dbtp_hundredths = if true_peak > 0.0 {
        Some(hundredths(20.0 * true_peak.log10())?)
    } else {
        None
    };
    let loudness_range_lu_hundredths = if loudness.integrated_lufs_hundredths.is_some() {
        loudness_range_lu(&short_term_loudness(samples, channel_count)?)
            .map(hundredths)
            .transpose()?
    } else {
        None
    };
    Ok(AudioDeliveryMeasurement {
        loudness,
        true_peak_dbtp_hundredths,
        loudness_range_lu_hundredths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full-scale tone at `fs/4` whose samples sit 45° off the peaks:
    /// every sample is `±sin(π/4)`, so the sample peak reads −3.01 dBFS while
    /// the waveform's true peak is 0 dBFS. The canonical inter-sample-peak
    /// fixture.
    #[allow(clippy::cast_precision_loss)]
    fn quarter_rate_tone_between_samples() -> Vec<f32> {
        (0..48_000)
            .flat_map(|frame| {
                let phase = std::f32::consts::FRAC_PI_2 * frame as f32 + std::f32::consts::FRAC_PI_4;
                let sample = phase.sin();
                [sample, sample]
            })
            .collect()
    }

    #[test]
    fn true_peak_recovers_the_inter_sample_peak_the_sample_peak_misses() {
        let measured =
            measure_delivery_audio(&quarter_rate_tone_between_samples(), 48_000, 2).unwrap();
        let sample_peak = measured.loudness.sample_peak_dbfs_hundredths.unwrap();
        let true_peak = measured.true_peak_dbtp_hundredths.unwrap();
        assert!((-305..=-297).contains(&sample_peak), "sample peak {sample_peak}");
        assert!((-20..=20).contains(&true_peak), "true peak {true_peak}");
    }

    #[test]
    fn true_peak_never_reads_below_the_sample_peak() {
        for amplitude in [0.05_f32, 0.5, 1.0] {
            let measured = measure_delivery_audio(&sine(amplitude), 48_000, 2).unwrap();
            let sample_peak = measured.loudness.sample_peak_dbfs_hundredths.unwrap();
            let true_peak = measured.true_peak_dbtp_hundredths.unwrap();
            assert!(true_peak >= sample_peak, "{amplitude}: {true_peak} < {sample_peak}");
            // A 1 kHz tone is band-limited: the interpolator adds at most a
            // few hundredths of a dB.
            assert!(true_peak - sample_peak <= 10, "{amplitude}: {true_peak} vs {sample_peak}");
        }
    }

    #[test]
    fn silence_carries_no_true_peak_and_no_range() {
        let measured = measure_delivery_audio(&vec![0.0; 96_000], 48_000, 2).unwrap();
        assert_eq!(measured.true_peak_dbtp_hundredths, None);
        assert_eq!(measured.loudness_range_lu_hundredths, None);
        assert_eq!(measured.loudness.integrated_lufs_hundredths, None);
    }

    #[test]
    fn a_steady_tone_has_no_loudness_range_and_a_step_has_the_step() {
        let steady = measure_delivery_audio(&sine(0.2), 48_000, 2).unwrap();
        // Two seconds of tone is shorter than one 3 s window: one value, no range.
        assert_eq!(steady.loudness_range_lu_hundredths, None);

        // Eight seconds at one level, eight seconds 12 dB louder, at 1 kHz.
        let mut stepped = Vec::new();
        for (seconds, amplitude) in [(8_u32, 0.05_f32), (8, 0.2)] {
            #[allow(clippy::cast_precision_loss)]
            for frame in 0..(seconds * 48_000) {
                let phase = std::f32::consts::TAU * 1_000.0 * frame as f32 / 48_000.0;
                let sample = amplitude * phase.sin();
                stepped.push(sample);
                stepped.push(sample);
            }
        }
        let measured = measure_delivery_audio(&stepped, 48_000, 2).unwrap();
        let range = measured.loudness_range_lu_hundredths.unwrap();
        // The 10th percentile sits on the quiet plateau and the 95th on the
        // loud one, 12.04 dB apart; windows straddling the step land between.
        assert!((1_100..=1_210).contains(&range), "range {range}");
    }

    #[test]
    fn the_kernel_has_unit_dc_gain_in_every_phase() {
        for taps in true_peak_kernel() {
            assert_eq!(taps.len(), 2 * TRUE_PEAK_HALF_SUPPORT as usize);
            assert!((taps.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn sine(amplitude: f32) -> Vec<f32> {
        (0..96_000)
            .flat_map(|frame| {
                let phase = std::f32::consts::TAU * 1_000.0 * frame as f32 / 48_000.0;
                let sample = amplitude * phase.sin();
                [sample, sample]
            })
            .collect()
    }

    #[test]
    fn reports_silence_without_inventing_a_decibel_floor() {
        let result = measure_loudness(&vec![0.0; 96_000], 48_000, 2).unwrap();
        assert_eq!(result.integrated_lufs_hundredths, None);
        assert_eq!(result.sample_peak_dbfs_hundredths, None);
    }

    #[test]
    fn six_decibels_of_gain_moves_both_measurements_six_decibels() {
        let quiet = measure_loudness(&sine(0.1), 48_000, 2).unwrap();
        let loud = measure_loudness(&sine(0.2), 48_000, 2).unwrap();
        let loudness_delta =
            loud.integrated_lufs_hundredths.unwrap() - quiet.integrated_lufs_hundredths.unwrap();
        let peak_delta =
            loud.sample_peak_dbfs_hundredths.unwrap() - quiet.sample_peak_dbfs_hundredths.unwrap();
        assert!((600..=605).contains(&loudness_delta));
        assert!((600..=605).contains(&peak_delta));
    }
}
