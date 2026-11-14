//! Temporary level measurement; DQ2 replaces this with cached evidence.
use super::{Error, PiecePolicy, SAMPLE_RATE};

fn sample_count(micros: u64) -> Result<usize, Error> {
    usize::try_from(u128::from(micros) * u128::from(SAMPLE_RATE) / 1_000_000)
        .map_err(|_| Error::InvalidPolicy("sample count overflow"))
}
impl PiecePolicy {
    pub(super) fn buffer_samples(&self) -> Result<usize, Error> {
        if self.target_us == 0 || self.search_us >= self.target_us {
            return Err(Error::InvalidPolicy(
                "target must be positive and greater than search span",
            ));
        }
        let maximum = self
            .target_us
            .checked_add(self.search_us)
            .ok_or(Error::InvalidPolicy("window overflow"))?;
        let samples = sample_count(maximum)?;
        if samples == 0 {
            return Err(Error::InvalidPolicy("window needs at least one sample"));
        }
        Ok(samples)
    }
}

pub(super) fn quiet_point(samples: &[f32], policy: &PiecePolicy) -> Result<usize, Error> {
    policy.buffer_samples()?;
    let window = sample_count(policy.level_window_us)?;
    let quiet_samples = sample_count(policy.quiet_span_us)?;
    if window == 0 || quiet_samples == 0 {
        return Err(Error::InvalidPolicy(
            "level window and quiet span need at least one sample",
        ));
    }
    let span = quiet_samples.div_ceil(window);
    let nominal = sample_count(policy.target_us)?.min(samples.len());
    let search = sample_count(policy.search_us)?;
    let start = nominal.saturating_sub(search);
    let end = nominal.saturating_add(search).min(samples.len());
    let available = (end - start) / window;
    if available < span {
        return Ok(nominal);
    }
    let levels = samples[start..end]
        .chunks_exact(window)
        .map(|chunk| chunk.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>())
        .collect::<Vec<_>>();
    let mut best = None;
    let mut energy = levels[..span].iter().sum::<f64>();
    for from in 0..=levels.len() - span {
        let centre = start + from * window + span * window / 2;
        let distance = centre.abs_diff(nominal);
        if best
            .is_none_or(|(level, gap, _)| energy.total_cmp(&level).then(distance.cmp(&gap)).is_lt())
        {
            best = Some((energy, distance, centre));
        }
        if from + span < levels.len() {
            energy += levels[from + span] - levels[from];
        }
    }
    Ok(best.map_or(nominal, |(_, _, centre)| centre))
}
