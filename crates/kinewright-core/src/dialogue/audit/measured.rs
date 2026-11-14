// Derived from Scarlet packages/mcp/src/timeline-perception.ts and gap-sound.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{AuditSettings, Level};
use crate::dialogue::{Dialogue, Hop};

pub(super) struct Sample {
    pub start: u64,
    pub end: u64,
    pub center: u64,
    pub hop: Hop,
}

pub(super) fn samples(
    recording: &Dialogue,
    start: u64,
    end: u64,
) -> impl DoubleEndedIterator<Item = Sample> {
    let profile = recording.profile();
    let step = u64::from(profile.hop_micros());
    let first = usize::try_from(start / step)
        .unwrap_or(profile.len())
        .min(profile.len());
    let last = usize::try_from(end.div_ceil(step))
        .unwrap_or(profile.len())
        .min(profile.len());
    (first..last).filter_map(move |i| {
        let span = profile.span(i);
        let a = start.max(span.start_micros());
        let b = end.min(span.end_micros());
        (a < b).then(|| Sample {
            start: a,
            end: b,
            center: u64::midpoint(span.start_micros(), span.end_micros()),
            hop: profile.hop(i).expect("validated profile index"),
        })
    })
}

pub(super) fn level(recording: &Dialogue, start: u64, end: u64) -> Option<Level> {
    let mut sum = 0_i128;
    let mut duration = 0_u64;
    let mut peak = crate::dialogue::LEVEL_FLOOR;
    for s in samples(recording, start, end) {
        let span = s.end - s.start;
        duration += span;
        sum += i128::from(s.hop.rms_dbfs_hundredths) * i128::from(span);
        peak = peak.max(s.hop.peak_dbfs_hundredths);
    }
    if duration == 0 {
        return None;
    }
    let mean = -((-sum + i128::from(duration) / 2) / i128::from(duration));
    Some(Level {
        us: [start, end],
        rms_cdb: i16::try_from(mean).expect("mean of validated nonpositive levels"),
        peak_cdb: peak,
    })
}

pub(super) fn low(hop: Hop, settings: &AuditSettings) -> bool {
    hop.rms_dbfs_hundredths <= settings.quiet_rms_cdb
        && hop.peak_dbfs_hundredths <= settings.quiet_peak_cdb
}

pub(super) fn air(
    recording: &Dialogue,
    start: u64,
    end: u64,
    outgoing: bool,
    voice_matters: bool,
    settings: &AuditSettings,
) -> u64 {
    let quiet = |s: &Sample| low(s.hop, settings) && (!voice_matters || !s.hop.voice);
    if outgoing {
        samples(recording, start, end)
            .rev()
            .take_while(quiet)
            .map(|s| s.end - s.start)
            .sum()
    } else {
        samples(recording, start, end)
            .take_while(quiet)
            .map(|s| s.end - s.start)
            .sum()
    }
}

pub(super) fn percentile(mut values: Vec<i16>, percent: u8) -> Option<i16> {
    if values.is_empty() {
        return None;
    }
    let index = (values.len() / 100 * usize::from(percent)
        + (values.len() % 100 * usize::from(percent)) / 100)
        .min(values.len() - 1);
    Some(*values.select_nth_unstable(index).1)
}

pub(super) fn speaking_level(recording: &Dialogue, settings: &AuditSettings) -> i16 {
    percentile(
        recording
            .profile()
            .hops()
            .map(|h| h.rms_dbfs_hundredths)
            .collect(),
        settings.speaking_percentile,
    )
    .expect("validated complete nonempty profile")
}

pub(super) fn phrase_level(
    recording: &Dialogue,
    from: usize,
    to: usize,
    start: u64,
    end: u64,
    settings: &AuditSettings,
) -> Option<i16> {
    let words = &recording.words()[from..=to];
    let mut levels = Vec::new();
    let mut duration = 0_u64;
    for s in samples(recording, start, end) {
        if start <= s.center
            && s.center < end
            && words
                .iter()
                .any(|w| w.is_spoken() && w.start_micros <= s.center && s.center < w.end_micros)
        {
            levels.push(s.hop.rms_dbfs_hundredths);
            duration += s.end - s.start;
        }
    }
    if duration < settings.min_sound_run_us {
        None
    } else {
        percentile(levels, settings.speaking_percentile)
    }
}
