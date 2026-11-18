use std::{collections::BTreeSet, ops::Range};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AssetId, ClipId};

pub const IMPORT_RECORD_MAX_BYTES: usize = 4 * 1024;
pub const IMPORT_RECORD_MAX_ROWS: usize = 8;
pub const IMPORT_RECORD_MAX_ESTIMATED_RANGES: usize = 8;
pub const DOCUMENT_MAX_IMPORT_RECORDS: usize = 16;
pub const TRACK_NAME_MAX_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InterchangeFormat {
    Otio,
    Fcpxml,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum LossCode {
    SubframeRounded,
    CutSnapped,
    ClipTooShort,
    OutsideWindow,
    BeforeStartTrimmed,
    NestFlattened,
    TransitionLane,
    LaneAdded,
    MulticamActiveAngle,
    AuditionActivePick,
    SpeedRounded,
    SpeedClamped,
    ReverseDropped,
    VariableSpeedAveraged,
    NestRetimeUnsupported,
    RetimedAudioMuted,
    TransitionTypeMapped,
    TransitionAtNestCut,
    AudioCrossfadeLinear,
    TransitionHandlesShort,
    TransitionFadeClamped,
    GainClamped,
    GainKeysMapped,
    FadeShapeLinear,
    PanDropped,
    ChannelMapDropped,
    AvLinkUnpaired,
    MarkerDurationDropped,
    MarkerColorAsLabel,
    MarkerColorDropped,
    ItemMarkerPinned,
    MarkerOutsideItem,
    KeywordRatingDropped,
    EffectDropped,
    TitleStyleDefault,
    TitleTextTruncated,
    GeneratorDropped,
    ImageSequenceDropped,
    UnknownItem,
    SourceClamped,
    SourceOriginAssumed,
    MediaRangesMerged,
    StartRebased,
    RateSnapped,
    ResolutionInferred,
    DropFrameLabels,
    ColorSpaceDefault,
    TrackNamesDropped,
    ClipNamesDropped,
    MetadataDropped,
    TrailingGapTrimmed,
    MediaOffline,
    MediaTimeout,
    RemoteUrl,
    RelinkedByName,
    TimeWarpReading,
    HoldReading,
    AnchorRetimeReading,
}

impl LossCode {
    #[must_use]
    pub const fn is_item(self) -> bool {
        !matches!(
            self,
            Self::StartRebased
                | Self::RateSnapped
                | Self::ResolutionInferred
                | Self::DropFrameLabels
                | Self::ColorSpaceDefault
                | Self::TrackNamesDropped
                | Self::ClipNamesDropped
                | Self::MetadataDropped
                | Self::TrailingGapTrimmed
                | Self::MediaOffline
                | Self::MediaTimeout
                | Self::RemoteUrl
                | Self::RelinkedByName
                | Self::TimeWarpReading
                | Self::HoldReading
                | Self::AnchorRetimeReading
        )
    }

    #[must_use]
    pub const fn is_unverified(self) -> bool {
        matches!(
            self,
            Self::TimeWarpReading | Self::HoldReading | Self::AnchorRetimeReading
        )
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Fidelity {
    Approximated,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "unit", content = "value")]
pub enum LossBound {
    Frames(u64),
    SpeedTenthPercent(u64),
    TenthDb(u64),
    Milliseconds(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LossSite {
    pub source: String,
    pub frame: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LossRow {
    pub code: LossCode,
    pub fidelity: Fidelity,
    pub count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound: Option<LossBound>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unverified: bool,
    pub names: Vec<String>,
    pub at: Vec<LossSite>,
}

impl LossRow {
    /// The ordering used both to display rows and to choose which rows to keep.
    #[must_use]
    pub const fn order(&self) -> (u8, std::cmp::Reverse<u32>, LossCode) {
        let group = match (self.code, self.fidelity) {
            (code, Fidelity::Dropped) if code.is_item() => 0,
            (LossCode::MediaOffline | LossCode::MediaTimeout | LossCode::RemoteUrl, _) => 1,
            (code, _) if code.is_item() => 2,
            _ => 3,
        };
        (group, std::cmp::Reverse(self.count), self.code)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MediaCounts {
    pub assets: u32,
    pub online: u32,
    pub offline: u32,
    pub remote: u32,
    pub timed_out: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportRecord {
    pub format: InterchangeFormat,
    pub format_version: String,
    pub source_name: String,
    pub source_sha256: String,
    pub start_offset_frames: i64,
    pub items: u32,
    pub exact: u32,
    pub approximated: u32,
    pub dropped: u32,
    pub media: MediaCounts,
    pub estimated_assets: Vec<Range<AssetId>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retimed_source_end_drift_frames: Option<u64>,
    pub rows: Vec<LossRow>,
    pub omitted_codes: Vec<LossCode>,
}

impl ImportRecord {
    /// Hash the bytes already read for the import record.
    #[must_use]
    pub fn hash_source(bytes: &[u8]) -> String {
        kinewright_hash::sha256_bytes(bytes)
    }

    #[must_use]
    pub fn estimated_asset(&self, asset: AssetId) -> bool {
        self.estimated_assets
            .iter()
            .any(|range| range.contains(&asset))
    }

    /// # Errors
    ///
    /// Refuses inconsistent counts, duplicate or oversized fields, and oversized JSON.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.format_version.is_empty()
            || self.format_version.len() > 16
            || self.source_name.is_empty()
            || self.source_name.len() > 128
            || self.source_name.contains(['/', '\\'])
            || matches!(self.source_name.as_str(), "." | "..")
            || self.source_sha256.len() != 64
            || !self
                .source_sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("invalid import source");
        }
        if u64::from(self.exact) + u64::from(self.approximated) + u64::from(self.dropped)
            != u64::from(self.items)
        {
            return Err("inconsistent item totals");
        }
        if u64::from(self.media.online) + u64::from(self.media.offline)
            != u64::from(self.media.assets)
            || u64::from(self.media.remote) + u64::from(self.media.timed_out)
                > u64::from(self.media.offline)
        {
            return Err("inconsistent media totals");
        }

        self.validate_estimated_assets()?;
        self.validate_rows()?;
        if serde_json::to_vec(self)
            .map_err(|_| "invalid import record JSON")?
            .len()
            > IMPORT_RECORD_MAX_BYTES
        {
            return Err("import record exceeds 4 KiB");
        }
        Ok(())
    }

    fn validate_estimated_assets(&self) -> Result<(), &'static str> {
        if self.estimated_assets.len() > IMPORT_RECORD_MAX_ESTIMATED_RANGES {
            return Err("too many estimated asset ranges");
        }
        let mut next = AssetId(1);
        let mut count = 0_u64;
        for range in &self.estimated_assets {
            if range.start < next || range.is_empty() {
                return Err("overlapping, unsorted or empty estimated asset ranges");
            }
            next = range.end;
            count = count
                .checked_add(range.end.0 - range.start.0)
                .ok_or("too many estimated assets")?;
        }
        if count != u64::from(self.media.offline) {
            return Err("inconsistent estimated asset count");
        }
        Ok(())
    }

    fn validate_rows(&self) -> Result<(), &'static str> {
        if self.rows.len() > IMPORT_RECORD_MAX_ROWS || !self.rows.is_sorted_by_key(LossRow::order) {
            return Err("too many or unsorted import rows");
        }
        let mut codes = BTreeSet::new();
        for row in &self.rows {
            if !codes.insert(row.code)
                || row.count == 0
                || row.unverified != row.code.is_unverified()
                || row.names.len() > 2
                || row.names.iter().any(|name| name.len() > 24)
                || row.at.len() > 2
            {
                return Err("invalid import loss row");
            }
        }
        if !self.omitted_codes.iter().all(|code| codes.insert(*code)) {
            return Err("duplicate import loss code");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, OpError, Track, TrackId, TrackKind};

    fn record() -> ImportRecord {
        ImportRecord {
            format: InterchangeFormat::Otio,
            format_version: "Timeline.1".into(),
            source_name: "source.otio".into(),
            source_sha256: ImportRecord::hash_source(b"test"),
            start_offset_frames: 0,
            items: 1,
            exact: 1,
            approximated: 0,
            dropped: 0,
            media: MediaCounts::default(),
            estimated_assets: Vec::new(),
            retimed_source_end_drift_frames: None,
            rows: Vec::new(),
            omitted_codes: Vec::new(),
        }
    }

    #[test]
    fn document_rejects_an_inconsistent_or_oversized_decoded_import_record() {
        let mut document = Document {
            imported_from: vec![record()],
            ..Document::default()
        };
        let load = |document: &Document| {
            serde_json::from_slice::<Document>(&serde_json::to_vec(document).unwrap())
                .unwrap()
                .validate()
        };
        assert_eq!(load(&document), Ok(()));
        document.imported_from[0].dropped = 1;
        assert_eq!(
            load(&document),
            Err(OpError::InvalidImportRecord("inconsistent item totals"))
        );
        document.imported_from[0].dropped = 0;
        document.imported_from[0].rows.push(LossRow {
            code: LossCode::MetadataDropped,
            fidelity: Fidelity::Dropped,
            count: 1,
            bound: None,
            unverified: false,
            names: Vec::new(),
            at: vec![LossSite {
                source: "\u{0001}".repeat(1024),
                frame: 0,
                clip: None,
            }],
        });
        assert_eq!(
            load(&document),
            Err(OpError::InvalidImportRecord("import record exceeds 4 KiB"))
        );
    }

    #[test]
    fn track_name_limit_measures_utf8_bytes_and_absent_fields_are_omitted() {
        let mut document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: true,
                clips: Vec::new(),
            }],
            ..Document::default()
        };
        let json = serde_json::to_value(&document).unwrap();
        assert!(json.get("imported_from").is_none());
        assert!(json["tracks"][0].get("name").is_none());
        document.tracks[0].name = Some("é".repeat(32));
        assert_eq!(document.validate(), Ok(()));
        document.tracks[0].name = Some("é".repeat(33));
        assert_eq!(
            document.validate(),
            Err(OpError::TrackNameTooLong(TrackId(1)))
        );
    }
}
