use openreel_core::{AudioLoudness, MediaError};

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

#[cfg(test)]
mod tests {
    use super::*;

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
