use std::collections::BTreeMap;

use kinewright_core::{
    ClipId, Fidelity, IMPORT_RECORD_MAX_BYTES, IMPORT_RECORD_MAX_ROWS, ImportRecord, LossBound,
    LossCode, LossRow, LossSite,
};

use crate::{Refusal, RefusalCode, ir::Loss, shorten};

/// Page up to 64 occurrence sites for one loss code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteQuery {
    pub code: LossCode,
    pub after: u32,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SitePage {
    pub sites: Vec<LossSite>,
    pub next: Option<u32>,
}

pub(crate) struct Report {
    rows: BTreeMap<LossCode, LossRow>,
    items: BTreeMap<String, (u32, Option<Fidelity>)>,
    query: Option<SiteQuery>,
    seen: u32,
    page: SitePage,
    pub retimed_drift: Option<u64>,
}

impl Report {
    pub fn new(query: Option<SiteQuery>) -> Self {
        Self {
            rows: BTreeMap::new(),
            items: BTreeMap::new(),
            query,
            seen: 0,
            page: SitePage::default(),
            retimed_drift: None,
        }
    }

    pub fn item(&mut self, source: &str, count: u32) {
        let item = self.items.entry(source.into()).or_default();
        item.0 = item.0.max(count);
    }

    pub fn mark_item(&mut self, source: &str, fidelity: Fidelity) {
        let item = self.items.entry(source.into()).or_default();
        item.1 = item.1.max(Some(fidelity));
    }

    pub fn is_empty(&self) -> bool {
        self.items.values().all(|(count, _)| *count == 0)
    }

    pub fn remap_clips(&mut self, ids: &BTreeMap<ClipId, ClipId>) {
        for site in self
            .rows
            .values_mut()
            .flat_map(|row| &mut row.at)
            .chain(&mut self.page.sites)
        {
            site.clip = site.clip.and_then(|id| ids.get(&id).copied());
        }
    }

    pub fn loss(&mut self, loss: &Loss, source: &str, frame: i64, clip: Option<ClipId>) {
        self.loss_many(loss, source, frame, clip, 1);
    }

    pub fn loss_many(
        &mut self,
        loss: &Loss,
        source: &str,
        frame: i64,
        clip: Option<ClipId>,
        count: u32,
    ) {
        if count == 0 {
            return;
        }
        if loss.code.is_item() {
            self.mark_item(source, loss.fidelity);
        }
        let row = self.rows.entry(loss.code).or_insert_with(|| LossRow {
            code: loss.code,
            fidelity: loss.fidelity,
            count: 0,
            bound: None,
            unverified: loss.code.is_unverified(),
            names: Vec::new(),
            at: Vec::new(),
        });
        row.fidelity = row.fidelity.max(loss.fidelity);
        row.count = row.count.saturating_add(count);
        row.bound = max_bound(row.bound, loss.bound);
        if let Some(name) = &loss.name {
            let name = shorten(name, 24);
            if row.names.len() < 2 && !row.names.iter().any(|existing| existing == name) {
                row.names.push(name.into());
            }
        }
        let site = || LossSite {
            source: source.into(),
            frame,
            clip,
        };
        if !source.is_empty() && row.at.len() < 2 {
            row.at.push(site());
        }
        if let Some(query) = self.query.filter(|query| query.code == loss.code)
            && !source.is_empty()
        {
            self.seen += 1;
            if self.seen > query.after {
                if self.page.sites.len() < 64 {
                    self.page.sites.push(site());
                } else {
                    self.page.next = Some(query.after + 64);
                }
            }
        }
    }

    pub fn finish(self, record: &mut ImportRecord) -> Result<SitePage, Refusal> {
        if self
            .items
            .values()
            .map(|(count, _)| u64::from(*count))
            .sum::<u64>()
            > u64::try_from(crate::MAX_ITEMS).map_err(|_| Refusal::arithmetic())?
        {
            return Err(crate::parse::too_many_items());
        }
        for (count, fidelity) in self.items.values() {
            match fidelity {
                None => record.exact += count,
                Some(Fidelity::Approximated) => record.approximated += count,
                Some(Fidelity::Dropped) => record.dropped += count,
            }
        }
        record.items = record.exact + record.approximated + record.dropped;
        record.retimed_source_end_drift_frames = self.retimed_drift;
        let mut rows: Vec<_> = self.rows.into_values().collect();
        rows.sort_by_key(LossRow::order);
        if rows.len() > IMPORT_RECORD_MAX_ROWS {
            record
                .omitted_codes
                .extend(rows.drain(IMPORT_RECORD_MAX_ROWS..).map(|row| row.code));
        }
        record.rows = rows;
        trim_record(record)?;
        record
            .validate()
            .map_err(|message| Refusal::new(RefusalCode::LoweringFailed, message))?;
        Ok(self.page)
    }
}

fn max_bound(a: Option<LossBound>, b: Option<LossBound>) -> Option<LossBound> {
    match (a, b) {
        (Some(LossBound::Frames(a)), Some(LossBound::Frames(b))) => {
            Some(LossBound::Frames(a.max(b)))
        }
        (Some(LossBound::SpeedTenthPercent(a)), Some(LossBound::SpeedTenthPercent(b))) => {
            Some(LossBound::SpeedTenthPercent(a.max(b)))
        }
        (Some(LossBound::TenthDb(a)), Some(LossBound::TenthDb(b))) => {
            Some(LossBound::TenthDb(a.max(b)))
        }
        (Some(LossBound::Milliseconds(a)), Some(LossBound::Milliseconds(b))) => {
            Some(LossBound::Milliseconds(a.max(b)))
        }
        (a, b) => a.or(b),
    }
}

fn trim_record(record: &mut ImportRecord) -> Result<(), Refusal> {
    while serde_json::to_vec(record)
        .map_err(|_| {
            Refusal::new(
                RefusalCode::LoweringFailed,
                "The import record cannot serialize.",
            )
        })?
        .len()
        > IMPORT_RECORD_MAX_BYTES
    {
        if let Some(row) = record
            .rows
            .iter_mut()
            .rev()
            .find(|row| !row.at.is_empty() || !row.names.is_empty())
        {
            if row.at.pop().is_none() {
                row.names.pop();
            }
        } else if let Some(row) = record.rows.pop() {
            record.omitted_codes.insert(0, row.code);
        } else {
            return Err(Refusal::new(
                RefusalCode::LoweringFailed,
                "The import record header exceeds 4 KiB.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use kinewright_core::{InterchangeFormat, MediaCounts};
    use proptest::prelude::*;

    use super::*;

    fn empty_record(source_name: String) -> ImportRecord {
        ImportRecord {
            format: InterchangeFormat::Otio,
            format_version: "Timeline.1".into(),
            source_name,
            source_sha256: ImportRecord::hash_source(b"record test"),
            start_offset_frames: 0,
            items: 0,
            exact: 0,
            approximated: 0,
            dropped: 0,
            media: MediaCounts::default(),
            estimated_assets: Vec::new(),
            retimed_source_end_drift_frames: None,
            rows: Vec::new(),
            omitted_codes: Vec::new(),
        }
    }

    const CODES: [LossCode; 10] = [
        LossCode::ClipTooShort,
        LossCode::OutsideWindow,
        LossCode::BeforeStartTrimmed,
        LossCode::ReverseDropped,
        LossCode::NestRetimeUnsupported,
        LossCode::RetimedAudioMuted,
        LossCode::EffectDropped,
        LossCode::GeneratorDropped,
        LossCode::ImageSequenceDropped,
        LossCode::UnknownItem,
    ];

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, ..ProptestConfig::default() })]

        #[test]
        fn worst_case_escaped_report_samples_keep_the_record_valid(
            control in 0_u8..=0x1f,
            site_len in 160_usize..512,
            repetitions in 1_u32..1_000,
        ) {
            let mut report = Report::new(None);
            let name = char::from(control).to_string().repeat(24);
            let mut record = empty_record(name.repeat(6).chars().take(128).collect());
            for (index, code) in CODES.into_iter().enumerate() {
                for sample in ["x","y"] {
                    let source = format!("{index}:{sample}-{}", name.repeat(site_len / name.len()));
                    report.item(&source, repetitions);
                    let mut label = name.clone();
                    label.replace_range(0..1, sample);
                    report.loss_many(&Loss::dropped(code).named(&label), &source, 0, None, repetitions);
                }
            }
            report.finish(&mut record).unwrap();
            let serialized = serde_json::to_vec(&record).unwrap();
            prop_assert!(serialized.len() <= IMPORT_RECORD_MAX_BYTES);
            prop_assert_eq!(record.validate(), Ok(()));
            prop_assert_eq!(record.items, 20 * repetitions);
            prop_assert_eq!(record.items, record.dropped);
            let got: std::collections::BTreeSet<_> = record.rows.iter().map(|row| row.code)
                .chain(record.omitted_codes.iter().copied()).collect();
            prop_assert_eq!(got, CODES.into_iter().collect());
            prop_assert!(record.omitted_codes.contains(&LossCode::UnknownItem));
        }
    }

    #[test]
    fn paging_keeps_the_requested_occurrences_beyond_the_record_samples() {
        let query = SiteQuery {
            code: LossCode::UnknownItem,
            after: 3,
        };
        let mut report = Report::new(Some(query));
        let mut record = empty_record("paging.otio".into());
        for occurrence in 0..80 {
            let site = format!("V1#{occurrence}");
            report.item(&site, 1);
            report.loss(
                &Loss::dropped(LossCode::UnknownItem),
                &site,
                occurrence,
                None,
            );
        }
        let page = report.finish(&mut record).unwrap();
        assert_eq!(record.rows[0].at.len(), 2);
        assert_eq!(page.sites.len(), 64);
        assert_eq!(page.sites[0].source, "V1#3");
        assert_eq!(page.sites[63].source, "V1#66");
        assert_eq!(page.next, Some(67));
    }
}
