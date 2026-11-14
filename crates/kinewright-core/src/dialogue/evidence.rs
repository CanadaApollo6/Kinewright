// Derived from Scarlet packages/mcp/src/dialogue-join.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{InputError, Settings, SoundProfile, Span, Word};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    Quiet,
    Valley,
    Sound,
    Unmeasured,
}

/// Prepared evidence for one complete, immutable recording measurement.
#[derive(Debug)]
pub struct Dialogue {
    pub(super) profile: SoundProfile,
    pub(super) words: Vec<Word>,
    pub(super) settings: Settings,
    quiet: Vec<Span>,
    valleys: Vec<Span>,
    pub(super) gaps: Vec<Span>,
    pub(super) dips: Vec<Span>,
}

fn append(runs: &mut Vec<Span>, span: Span) {
    if let Some(last) = runs
        .last_mut()
        .filter(|last| last.end_micros == span.start_micros)
    {
        last.end_micros = span.end_micros;
    } else {
        runs.push(span);
    }
}

impl Dialogue {
    /// Derive acoustic evidence once. Word intervals may overlap, but both starts
    /// and ends must be ordered. Transcript times never override measured silence.
    ///
    /// # Errors
    /// Refuses invalid settings, empty token text, unordered or out-of-profile times.
    pub fn new(
        profile: SoundProfile,
        words: Vec<Word>,
        settings: Settings,
    ) -> Result<Self, InputError> {
        settings.validate()?;
        for (index, word) in words.iter().enumerate() {
            if word.text.trim().is_empty()
                || word.start_micros >= word.end_micros
                || word.end_micros > profile.duration_micros()
                || index > 0
                    && (words[index - 1].start_micros > word.start_micros
                        || words[index - 1].end_micros > word.end_micros)
            {
                return Err(InputError::Word { index });
            }
        }
        let mut quiet = Vec::new();
        let mut valleys = Vec::new();
        let mut dips = Vec::new();
        for (index, hop) in profile.hops().enumerate() {
            let span = profile.span(index);
            if hop.rms_dbfs_hundredths <= settings.dip_dbfs_hundredths {
                append(&mut dips, span);
            }
            if !hop.voice
                && hop.rms_dbfs_hundredths <= settings.quiet_rms_max
                && hop.peak_dbfs_hundredths <= settings.quiet_peak_max
            {
                append(&mut quiet, span);
            }
            if hop.rms_dbfs_hundredths <= settings.valley_rms_max
                && hop.peak_dbfs_hundredths <= settings.valley_peak_max
            {
                append(&mut valleys, span);
            }
        }
        // These are small interval values, not copies of the recording profile.
        let mut gaps: Vec<_> = quiet
            .iter()
            .copied()
            .chain(
                valleys
                    .iter()
                    .copied()
                    .filter(|span| span.duration() >= settings.valley_pause_micros),
            )
            .collect();
        gaps.sort_by_key(|span| (span.start_micros, std::cmp::Reverse(span.end_micros)));
        gaps.dedup();
        Ok(Self {
            profile,
            words,
            settings,
            quiet,
            valleys,
            gaps,
            dips,
        })
    }
    #[must_use]
    pub const fn profile(&self) -> &SoundProfile {
        &self.profile
    }
    #[must_use]
    pub fn words(&self) -> &[Word] {
        &self.words
    }
    #[must_use]
    pub const fn settings(&self) -> &Settings {
        &self.settings
    }
    #[must_use]
    pub fn quiet_runs(&self) -> &[Span] {
        &self.quiet
    }
    #[must_use]
    pub fn valleys(&self) -> &[Span] {
        &self.valleys
    }

    fn words_overlapping(&self, span: Span) -> &[Word] {
        let first = self
            .words
            .partition_point(|w| w.end_micros <= span.start_micros);
        let last = self
            .words
            .partition_point(|w| w.start_micros < span.end_micros);
        &self.words[first..last]
    }
    pub(super) fn interior_word(&self, n: u128, d: u128) -> Option<(usize, &Word)> {
        let first = self
            .words
            .partition_point(|w| u128::from(w.end_micros) * d <= n);
        self.words[first..]
            .iter()
            .enumerate()
            .take_while(|(_, w)| u128::from(w.start_micros) * d < n)
            .find(|(_, w)| {
                let lo = w
                    .start_micros
                    .saturating_add(self.settings.word_margin_micros);
                let hi = w
                    .end_micros
                    .saturating_sub(self.settings.word_margin_micros);
                w.is_spoken() && u128::from(lo) * d < n && n < u128::from(hi) * d
            })
            .map(|(index, word)| (first + index, word))
    }
    pub(super) fn quiet_support(&self, span: Span, at: u64) -> Option<(u64, u64)> {
        let margin = self.settings.quiet_margin_micros;
        if span.duration() < margin * 2 {
            return None;
        }
        let mut lo = span.start_micros + margin;
        let mut hi = span.end_micros - margin;
        if at < lo || at > hi {
            return None;
        }
        if span.duration() < self.settings.in_word_gap_micros {
            for word in self
                .words_overlapping(span)
                .iter()
                .filter(|w| w.is_spoken())
            {
                let a = word
                    .start_micros
                    .saturating_add(self.settings.word_margin_micros);
                let b = word
                    .end_micros
                    .saturating_sub(self.settings.word_margin_micros);
                if a >= b {
                    continue;
                }
                if b <= at {
                    lo = lo.max(b);
                } else if a >= at {
                    hi = hi.min(a);
                } else {
                    return None;
                }
            }
        }
        (lo <= hi).then_some((lo, hi))
    }
    pub(super) fn quiet_at_ratio(&self, n: u128, d: u128) -> bool {
        self.gaps.iter().any(|q| {
            let m = self.settings.quiet_margin_micros;
            q.duration() >= m * 2
                && u128::from(q.start_micros + m) * d <= n
                && n <= u128::from(q.end_micros - m) * d
                && (q.duration() >= self.settings.in_word_gap_micros
                    || self.interior_word(n, d).is_none())
        })
    }
    #[must_use]
    pub fn sound_at(&self, at_micros: u64) -> Sound {
        self.sound_ratio(u128::from(at_micros), 1)
    }
    pub(super) fn sound_ratio(&self, n: u128, d: u128) -> Sound {
        if self.profile.index_at(n, d).is_none() {
            return Sound::Unmeasured;
        }
        if self.quiet_at_ratio(n, d) {
            return Sound::Quiet;
        }
        let margin = self.settings.quiet_margin_micros;
        if self.gaps.iter().chain(&self.valleys).any(|q| {
            q.duration() >= margin * 2
                && u128::from(q.start_micros + margin) * d <= n
                && n <= u128::from(q.end_micros - margin) * d
        }) {
            Sound::Valley
        } else {
            Sound::Sound
        }
    }
    pub(super) fn nearest_quiet(&self, at: u64) -> (Option<u64>, Option<u64>) {
        let mut before = None;
        let mut after = None;
        for q in &self.gaps {
            let margin = self.settings.quiet_margin_micros;
            if q.duration() < margin * 2 {
                continue;
            }
            let lo = q.start_micros + margin;
            let hi = q.end_micros - margin;
            let mut left = at.min(hi);
            let mut right = at.max(lo);
            if q.duration() < self.settings.in_word_gap_micros {
                for word in self
                    .words_overlapping(*q)
                    .iter()
                    .rev()
                    .filter(|w| w.is_spoken())
                {
                    let a = word
                        .start_micros
                        .saturating_add(self.settings.word_margin_micros);
                    let b = word
                        .end_micros
                        .saturating_sub(self.settings.word_margin_micros);
                    if a < left && left < b {
                        left = a;
                    }
                }
                for word in self.words_overlapping(*q).iter().filter(|w| w.is_spoken()) {
                    let a = word
                        .start_micros
                        .saturating_add(self.settings.word_margin_micros);
                    let b = word
                        .end_micros
                        .saturating_sub(self.settings.word_margin_micros);
                    if a < right && right < b {
                        right = b;
                    }
                }
            }
            if left >= lo && self.quiet_support(*q, left).is_some() {
                before = Some(before.map_or(left, |v: u64| v.max(left)));
            }
            if right <= hi && self.quiet_support(*q, right).is_some() {
                after = Some(after.map_or(right, |v: u64| v.min(right)));
            }
        }
        (before, after)
    }
}
