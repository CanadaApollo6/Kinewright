//! Temporary 10 ms level measurement; DQ2 replaces this with cached evidence.
use super::SAMPLE_RATE;
pub(super) fn quiet_point(samples: &[f32]) -> usize {
    let rate = usize::try_from(SAMPLE_RATE).unwrap_or(16_000);
    let window = rate / 100;
    let span = 30; // A 300 ms quiet region avoids choosing a single stop consonant.
    let nominal = (60 * rate).min(samples.len());
    let start = nominal.saturating_sub(5 * rate);
    let end = (nominal + 5 * rate).min(samples.len());
    if end - start < span * window {
        return nominal;
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
    best.map_or(nominal, |(_, _, centre)| centre)
}
