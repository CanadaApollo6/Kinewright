//! Typed capacity bounds and the single candidate copy that preserves capacity.
use crate::undo::Error;
use crate::{
    AssetId, AudioBus, AudioBusId, AudioMaster, AudioMix, AutomationCurve, BinId, BlendMode, Clip,
    ClipContent, ClipId, ColorContext, ColorDescription, Document, Effect, EffectId, Fidelity,
    FreezeFrame, ImportRecord, InterchangeFormat, InvestigatorPreferences, Keyframe, LinkId,
    LossBound, LossCode, LossRow, LossSite, LutAsset, LutAssetId, LutAssetKind, LutAssetSource,
    Marker, MarkerId, MediaAsset, MediaBin, MediaCatalog, MediaCounts, MediaKind,
    MediaSourceFingerprint, Operation, PanLaw, ParamValue, Rational, RelinkCandidate,
    RemoteLocator, SolidColor, SourceSelect, StringOut, StringOutId, SyncGroup, SyncGroupId,
    SyncGroupMember, TimeCode, Title, Track, TrackId, TrackKind, TrackMix, Transition,
};
use std::{collections::BTreeMap, mem::size_of, path::PathBuf};
/// Source capacity and canonical wire replay capacity are both admitted.
#[derive(Clone, Copy)]
pub(crate) enum Allocation {
    Actual,
    Canonical,
    ReplaySafe,
}
fn slots(len: usize, capacity: usize, layout: Allocation, minimum: usize) -> usize {
    if matches!(layout, Allocation::Actual) {
        return capacity;
    }
    let canonical = if len == 0 {
        0
    } else {
        len.checked_next_power_of_two()
            .unwrap_or(usize::MAX)
            .max(minimum)
    };
    match layout {
        Allocation::Actual => capacity,
        Allocation::Canonical => canonical,
        Allocation::ReplaySafe => capacity.max(canonical),
    }
}
pub(crate) trait Heap {
    fn measured(&self, layout: Allocation) -> Result<u64, Error>;
    fn heap(&self) -> Result<u64, Error> {
        self.measured(Allocation::Actual)
    }
}
fn sum(values: impl IntoIterator<Item = Result<u64, Error>>) -> Result<u64, Error> {
    values
        .into_iter()
        .try_fold(0u64, |n, v| n.checked_add(v?).ok_or(Error::Capacity))
}
fn allocation<T>(n: usize) -> Result<u64, Error> {
    (n as u64)
        .checked_mul(size_of::<T>() as u64)
        .ok_or(Error::Capacity)
}
trait Preserve: Heap + Sized {
    fn copied(&self, layout: Allocation) -> Self;
}
fn inline_copy<T: Clone>(value: &T) -> T {
    value.clone()
}
macro_rules! inline {($($t:ty),*$(,)?)=>{$(
impl Heap for $t {fn measured(&self, _layout: Allocation) ->Result<u64,Error>{const {assert!(!std::mem::needs_drop::<$t>(),"owned field must be charged");} Ok(0)}}
impl Preserve for $t {fn copied(&self, _layout: Allocation)->Self{inline_copy(self)}}
)*};}
inline!(
    bool,
    u8,
    u32,
    u64,
    i32,
    i64,
    usize,
    AssetId,
    TrackId,
    ClipId,
    MarkerId,
    EffectId,
    BinId,
    StringOutId,
    SyncGroupId,
    AudioBusId,
    LutAssetId,
    LinkId,
    TimeCode,
    Rational,
    MediaKind,
    TrackKind,
    ColorDescription,
    ColorContext,
    BlendMode,
    PanLaw,
    LutAssetKind,
    SolidColor,
    FreezeFrame,
    InterchangeFormat,
    LossCode,
    Fidelity,
    LossBound,
    MediaCounts,
    crate::title::TitlePosition,
    crate::CaptionPreset,
    crate::automation::KeyframeInterpolation,
    crate::editorial::ThreePointMode
);
impl<T: Heap> Heap for Option<T> {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        self.as_ref().map_or(Ok(0), |value| value.measured(layout))
    }
}
impl<T: Heap> Heap for Vec<T> {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        sum([
            allocation::<T>(slots(
                self.len(),
                self.capacity(),
                layout,
                if size_of::<T>() == 1 {
                    8
                } else if size_of::<T>() <= 1024 {
                    4
                } else {
                    1
                },
            ))?,
            u64::from(
                slots(
                    self.len(),
                    self.capacity(),
                    layout,
                    if size_of::<T>() == 1 {
                        8
                    } else if size_of::<T>() <= 1024 {
                        4
                    } else {
                        1
                    },
                ) > 0,
            ) * 16,
        ]
        .into_iter()
        .map(Ok)
        .chain(self.iter().map(|value| value.measured(layout))))
    }
}
impl Heap for String {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        let bytes = slots(self.len(), self.capacity(), layout, 8);
        Ok(bytes as u64 + u64::from(bytes > 0) * 16)
    }
}
impl Heap for PathBuf {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        let bytes = slots(self.as_os_str().len(), self.capacity(), layout, 8);
        Ok(bytes as u64 + u64::from(bytes > 0) * 16)
    }
}
impl<A: Heap, B: Heap> Heap for (A, B) {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        sum([self.0.measured(layout), self.1.measured(layout)])
    }
}
impl<T: Heap, const N: usize> Heap for [T; N] {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        sum(self.iter().map(|value| value.measured(layout)))
    }
}
impl<T: Heap> Heap for std::ops::Range<T> {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        sum([self.start.measured(layout), self.end.measured(layout)])
    }
}
impl<K: Heap, V: Heap> Heap for BTreeMap<K, V> {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        // A node has at most eleven entries and twelve child links. One node per
        // entry plus an empty root is conservative even after removals.
        let node = allocation::<(K, V)>(11)?
            .checked_add(128)
            .ok_or(Error::Capacity)?;
        let nodes = (self.len() as u64 + 1)
            .checked_mul(node)
            .ok_or(Error::Capacity)?;
        sum(std::iter::once(Ok(nodes)).chain(
            self.iter()
                .flat_map(|(k, v)| [k.measured(layout), v.measured(layout)]),
        ))
    }
}
impl<T: Preserve> Preserve for Option<T> {
    fn copied(&self, layout: Allocation) -> Self {
        self.as_ref().map(|value| value.copied(layout))
    }
}
impl<T: Preserve> Preserve for Vec<T> {
    fn copied(&self, layout: Allocation) -> Self {
        let mut result = Vec::with_capacity(slots(
            self.len(),
            self.capacity(),
            layout,
            if size_of::<T>() == 1 {
                8
            } else if size_of::<T>() <= 1024 {
                4
            } else {
                1
            },
        ));
        result.extend(self.iter().map(|value| value.copied(layout)));
        result
    }
}
impl Preserve for String {
    fn copied(&self, layout: Allocation) -> Self {
        let mut result = Self::with_capacity(slots(self.len(), self.capacity(), layout, 8));
        result.push_str(self);
        result
    }
}
impl Preserve for PathBuf {
    fn copied(&self, layout: Allocation) -> Self {
        let mut result = std::ffi::OsString::with_capacity(slots(
            self.as_os_str().len(),
            self.capacity(),
            layout,
            8,
        ));
        result.push(self.as_os_str());
        Self::from(result)
    }
}
impl<A: Preserve, B: Preserve> Preserve for (A, B) {
    fn copied(&self, layout: Allocation) -> Self {
        (self.0.copied(layout), self.1.copied(layout))
    }
}
impl<T: Preserve, const N: usize> Preserve for [T; N] {
    fn copied(&self, layout: Allocation) -> Self {
        std::array::from_fn(|i| self[i].copied(layout))
    }
}
impl<T: Preserve> Preserve for std::ops::Range<T> {
    fn copied(&self, layout: Allocation) -> Self {
        self.start.copied(layout)..self.end.copied(layout)
    }
}
impl<K: Preserve + Ord, V: Preserve> Preserve for BTreeMap<K, V> {
    fn copied(&self, layout: Allocation) -> Self {
        self.iter()
            .map(|(k, v)| (k.copied(layout), v.copied(layout)))
            .collect()
    }
}
macro_rules! fields{($t:ident{$($f:ident),*$(,)?})=>{
impl Heap for $t{fn measured(&self, layout: Allocation) ->Result<u64,Error>{let Self{$($f),*}=self;sum([$($f.measured(layout)),*])}}
impl Preserve for $t{fn copied(&self, layout: Allocation)->Self{let Self{$($f),*}=self;Self{$($f:$f.copied(layout)),*}}}
};}
impl Heap for RemoteLocator {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        self.0.measured(layout)
    }
}
impl Preserve for RemoteLocator {
    fn copied(&self, layout: Allocation) -> Self {
        Self::new(self.0.copied(layout))
    }
}
fields!(MediaAsset {
    id,
    path,
    remote_locator,
    name,
    duration,
    fps,
    kind,
    resolution,
    source_fingerprint,
    color_description,
    assumed_from
});
fields!(RelinkCandidate {
    path,
    fingerprint,
    kind,
    fps,
    duration,
    resolution
});
fields!(Effect {
    id,
    name,
    parameters,
    keyframes,
    enabled,
    enabled_curve
});
fields!(Clip {
    id,
    asset,
    source_range,
    content,
    timeline_start,
    effects,
    transition_in,
    link,
    audio_gain_tenth_db,
    audio_fade_in_frames,
    audio_fade_out_frames,
    audio_gain_curve,
    speed_percent,
    enabled,
    enabled_curve,
    blend_mode
});
fields!(Track {
    id,
    kind,
    name,
    sync_lock,
    clips
});
fields!(Marker {
    id,
    position,
    label,
    color_token
});
fields!(MediaBin {
    id,
    name,
    parent,
    assets
});
fields!(SourceSelect {
    asset,
    source,
    label
});
fields!(StringOut { id, name, selects });
fields!(SyncGroupMember {
    asset,
    offset,
    angle_name
});
fields!(SyncGroup { id, name, members });
fields!(MediaCatalog {
    bins,
    string_outs,
    sync_groups
});
fields!(AudioBus {
    id,
    name,
    tracks,
    gain_tenth_db,
    gain_curve,
    effects,
    ducking_sidechain_tracks
});
fields!(TrackMix {
    track,
    gain_tenth_db,
    pan_percent,
    gain_curve,
    pan_curve,
    mute,
    solo
});
fields!(AudioMaster {
    gain_tenth_db,
    gain_curve,
    effects
});
fields!(AudioMix {
    buses,
    tracks,
    master,
    pan_law
});
fields!(LutAsset {
    id,
    sha256,
    title,
    kind,
    size,
    byte_len,
    domain_min_millionths,
    domain_max_millionths,
    source
});
fields!(Document {
    tracks,
    media_pool,
    markers,
    catalog,
    audio_mix,
    color_context,
    lut_assets,
    investigator,
    imported_from,
    fps,
    resolution,
    duration
});
fields!(ImportRecord {
    format,
    format_version,
    source_name,
    source_sha256,
    start_offset_frames,
    items,
    exact,
    approximated,
    dropped,
    media,
    estimated_assets,
    retimed_source_end_drift_frames,
    rows,
    omitted_codes
});
fields!(LossRow {
    code,
    fidelity,
    count,
    bound,
    unverified,
    names,
    at
});
fields!(LossSite {
    source,
    frame,
    clip
});
fields!(MediaSourceFingerprint {
    content_sha256,
    byte_len
});
fields!(InvestigatorPreferences { muted_codes });
fields!(Title {
    text,
    font_size_token,
    color_token,
    position,
    background_scrim,
    fade_in_frames,
    fade_out_frames,
    caption_preset
});
fields!(AutomationCurve { keyframes });
fields!(Keyframe {
    at,
    value,
    interpolation,
    tangent_in,
    tangent_out
});
fields!(Transition { name, duration });
impl Heap for ParamValue {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        match self {
            Self::Text(v) => v.measured(layout),
            Self::Integer(_) | Self::Boolean(_) => Ok(0),
        }
    }
}
impl Heap for ClipContent {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        match self {
            Self::Title(v) => v.measured(layout),
            Self::Media | Self::Freeze(_) | Self::Adjustment | Self::Solid(_) => Ok(0),
        }
    }
}
impl Heap for LutAssetSource {
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        match self {
            Self::Imported { source_path } => source_path.measured(layout),
            Self::Builtin { name } => name.measured(layout),
        }
    }
}
impl Preserve for ParamValue {
    fn copied(&self, layout: Allocation) -> Self {
        match self {
            Self::Text(v) => Self::Text(v.copied(layout)),
            Self::Integer(v) => Self::Integer(*v),
            Self::Boolean(v) => Self::Boolean(*v),
        }
    }
}
impl Preserve for ClipContent {
    fn copied(&self, layout: Allocation) -> Self {
        match self {
            Self::Media => Self::Media,
            Self::Adjustment => Self::Adjustment,
            Self::Title(v) => Self::Title(v.copied(layout)),
            Self::Freeze(v) => Self::Freeze(v.copied(layout)),
            Self::Solid(v) => Self::Solid(v.copied(layout)),
        }
    }
}
impl Preserve for LutAssetSource {
    fn copied(&self, layout: Allocation) -> Self {
        match self {
            Self::Imported { source_path } => Self::Imported {
                source_path: source_path.copied(layout),
            },
            Self::Builtin { name } => Self::Builtin {
                name: name.copied(layout),
            },
        }
    }
}
impl Heap for Operation {
    #[allow(
        clippy::too_many_lines,
        reason = "every operation declares all owned fields in one exhaustive table"
    )]
    fn measured(&self, layout: Allocation) -> Result<u64, Error> {
        match self {
            Self::AddAsset { asset } => sum([asset.measured(layout)]),
            Self::RelinkAsset {
                asset,
                candidate,
                allow_unverified_source,
            } => sum([
                asset.measured(layout),
                candidate.measured(layout),
                allow_unverified_source.measured(layout),
            ]),
            Self::SetAssetColorDescription {
                asset,
                color_description,
            } => sum([asset.measured(layout), color_description.measured(layout)]),
            Self::SetColorContext { color_context } => sum([color_context.measured(layout)]),
            Self::UpsertBin { bin } => sum([bin.measured(layout)]),
            Self::RemoveBin { bin } => sum([bin.measured(layout)]),
            Self::SetAssetBin { asset, bin } => sum([asset.measured(layout), bin.measured(layout)]),
            Self::UpsertStringOut { string_out } => sum([string_out.measured(layout)]),
            Self::RemoveStringOut { string_out } => sum([string_out.measured(layout)]),
            Self::UpsertSyncGroup { sync_group } => sum([sync_group.measured(layout)]),
            Self::RemoveSyncGroup { sync_group } => sum([sync_group.measured(layout)]),
            Self::UpsertAudioBus { bus } => sum([bus.measured(layout)]),
            Self::RemoveAudioBus { bus } => sum([bus.measured(layout)]),
            Self::SetAudioMaster { master } => sum([master.measured(layout)]),
            Self::SetPanLaw { law } => sum([law.measured(layout)]),
            Self::AddTrack { track } => sum([track.measured(layout)]),
            Self::RemoveTrack { track } => sum([track.measured(layout)]),
            Self::SetTrackSyncLock { track, locked } => {
                sum([track.measured(layout), locked.measured(layout)])
            }
            Self::SetTrackMix {
                track,
                gain_tenth_db,
                pan_percent,
                mute,
                solo,
            } => sum([
                track.measured(layout),
                gain_tenth_db.measured(layout),
                pan_percent.measured(layout),
                mute.measured(layout),
                solo.measured(layout),
            ]),
            Self::SetTrackAutomation {
                track,
                parameter,
                curve,
            } => sum([
                track.measured(layout),
                parameter.measured(layout),
                curve.measured(layout),
            ]),
            Self::AddClip {
                track,
                asset,
                at,
                source,
            } => sum([
                track.measured(layout),
                asset.measured(layout),
                at.measured(layout),
                source.measured(layout),
            ]),
            Self::AddTitle {
                track,
                at,
                duration,
                title,
            } => sum([
                track.measured(layout),
                at.measured(layout),
                duration.measured(layout),
                title.measured(layout),
            ]),
            Self::SplitClip { clip, at } => sum([clip.measured(layout), at.measured(layout)]),
            Self::TrimClip { clip, new_source } => {
                sum([clip.measured(layout), new_source.measured(layout)])
            }
            Self::MoveClip { clip, to_track, to } => sum([
                clip.measured(layout),
                to_track.measured(layout),
                to.measured(layout),
            ]),
            Self::ThreePointEdit {
                track,
                asset,
                source_in,
                source_out,
                timeline_in,
                timeline_out,
                mode,
            } => sum([
                track.measured(layout),
                asset.measured(layout),
                source_in.measured(layout),
                source_out.measured(layout),
                timeline_in.measured(layout),
                timeline_out.measured(layout),
                mode.measured(layout),
            ]),
            Self::PatchedThreePointEdit {
                asset,
                source_in,
                source_out,
                timeline_in,
                timeline_out,
                mode,
                video_track,
                audio_track,
            } => sum([
                asset.measured(layout),
                source_in.measured(layout),
                source_out.measured(layout),
                timeline_in.measured(layout),
                timeline_out.measured(layout),
                mode.measured(layout),
                video_track.measured(layout),
                audio_track.measured(layout),
            ]),
            Self::SlipClip {
                clip,
                new_source_in,
            } => sum([clip.measured(layout), new_source_in.measured(layout)]),
            Self::RollEdit {
                left_clip,
                right_clip,
                to,
            } => sum([
                left_clip.measured(layout),
                right_clip.measured(layout),
                to.measured(layout),
            ]),
            Self::SlideClip { clip, to } => sum([clip.measured(layout), to.measured(layout)]),
            Self::ReplaceClip {
                clip,
                asset,
                source,
            }
            | Self::FitToFill {
                clip,
                asset,
                source,
            } => sum([
                clip.measured(layout),
                asset.measured(layout),
                source.measured(layout),
            ]),
            Self::DeleteClip { clip }
            | Self::RippleDeleteClip { clip }
            | Self::RemoveTransition { clip } => sum([clip.measured(layout)]),
            Self::RippleInsertGap {
                track,
                at,
                duration,
            } => sum([
                track.measured(layout),
                at.measured(layout),
                duration.measured(layout),
            ]),
            Self::LinkClips { clips } | Self::UnlinkClips { clips } => {
                sum([clips.measured(layout)])
            }
            Self::AddMarker { marker } => sum([marker.measured(layout)]),
            Self::RemoveMarker { marker } => sum([marker.measured(layout)]),
            Self::MoveMarker { marker, to } => sum([marker.measured(layout), to.measured(layout)]),
            Self::AddEffect { clip, effect } => {
                sum([clip.measured(layout), effect.measured(layout)])
            }
            Self::InsertEffect {
                clip,
                index,
                effect,
            } => sum([
                clip.measured(layout),
                index.measured(layout),
                effect.measured(layout),
            ]),
            Self::RemoveEffect { clip, effect } => {
                sum([clip.measured(layout), effect.measured(layout)])
            }
            Self::SetEffectParam {
                clip,
                effect,
                name,
                value,
            } => sum([
                clip.measured(layout),
                effect.measured(layout),
                name.measured(layout),
                value.measured(layout),
            ]),
            Self::SetEffectKeyframes {
                clip,
                effect,
                name,
                curve,
            } => sum([
                clip.measured(layout),
                effect.measured(layout),
                name.measured(layout),
                curve.measured(layout),
            ]),
            Self::ClearEffectKeyframes { clip, effect, name } => sum([
                clip.measured(layout),
                effect.measured(layout),
                name.measured(layout),
            ]),
            Self::UpsertEffectKeyframe {
                clip,
                effect,
                name,
                key,
            } => sum([
                clip.measured(layout),
                effect.measured(layout),
                name.measured(layout),
                key.measured(layout),
            ]),
            Self::RemoveEffectKeyframe {
                clip,
                effect,
                name,
                at,
            } => sum([
                clip.measured(layout),
                effect.measured(layout),
                name.measured(layout),
                at.measured(layout),
            ]),
            Self::SetEffectEnabled {
                clip,
                effect,
                enabled,
            } => sum([
                clip.measured(layout),
                effect.measured(layout),
                enabled.measured(layout),
            ]),
            Self::SetClipEnabled { clip, enabled } => {
                sum([clip.measured(layout), enabled.measured(layout)])
            }
            Self::SetClipEnabledCurve { clip, curve }
            | Self::SetClipGainEnvelope { clip, curve } => {
                sum([clip.measured(layout), curve.measured(layout)])
            }
            Self::CopyClipAttributes {
                from_clip,
                to_clip,
                names,
                include_keyframes,
            } => sum([
                from_clip.measured(layout),
                to_clip.measured(layout),
                names.measured(layout),
                include_keyframes.measured(layout),
            ]),
            Self::ConvertLegacyLook {
                clip,
                effect,
                lut_asset,
                mix_basis_points,
            } => sum([
                clip.measured(layout),
                effect.measured(layout),
                lut_asset.measured(layout),
                mix_basis_points.measured(layout),
            ]),
            Self::AddLutAsset { asset } => sum([asset.measured(layout)]),
            Self::RemoveLutAsset { lut_asset } => sum([lut_asset.measured(layout)]),
            Self::SetTitleParam { clip, name, value } => sum([
                clip.measured(layout),
                name.measured(layout),
                value.measured(layout),
            ]),
            Self::SetClipAudio {
                clip,
                gain_tenth_db,
                fade_in_frames,
                fade_out_frames,
            } => sum([
                clip.measured(layout),
                gain_tenth_db.measured(layout),
                fade_in_frames.measured(layout),
                fade_out_frames.measured(layout),
            ]),
            Self::AddTransition { clip, transition } => {
                sum([clip.measured(layout), transition.measured(layout)])
            }
            Self::SetMarkerParam {
                marker,
                name,
                value,
            } => sum([
                marker.measured(layout),
                name.measured(layout),
                value.measured(layout),
            ]),
            Self::AddFreezeFrame {
                track,
                at,
                duration,
                asset,
                source_frame,
            } => sum([
                track.measured(layout),
                at.measured(layout),
                duration.measured(layout),
                asset.measured(layout),
                source_frame.measured(layout),
            ]),
            Self::SetClipSpeed {
                clip,
                speed_percent,
            } => sum([clip.measured(layout), speed_percent.measured(layout)]),
            Self::SetClipBlendMode { clip, blend_mode } => {
                sum([clip.measured(layout), blend_mode.measured(layout)])
            }
            Self::AddAdjustmentClip {
                track,
                timeline_start,
                duration,
                effects,
            } => sum([
                track.measured(layout),
                timeline_start.measured(layout),
                duration.measured(layout),
                effects.measured(layout),
            ]),
            Self::AddSolidClip {
                track,
                timeline_start,
                duration,
                color,
            } => sum([
                track.measured(layout),
                timeline_start.measured(layout),
                duration.measured(layout),
                color.measured(layout),
            ]),
            Self::SetSolidColor { clip, color } => {
                sum([clip.measured(layout), color.measured(layout)])
            }
        }
    }
}
pub(crate) fn document_bytes(document: &Document) -> Result<u64, Error> {
    sum([
        Ok(size_of::<Document>() as u64 + 16),
        document.measured(Allocation::Actual),
    ])
}

/// Atomic candidate copy. Preserve capacities so in-place validated replay has
/// the same allocation bounds as admission, without a normalization copy.
pub(super) fn copy_document(document: &Document) -> Document {
    super::metrics::document_copy();
    document.copied(Allocation::Actual)
}

pub(super) fn replay_document_bytes(document: &Document) -> Result<u64, Error> {
    sum([
        Ok(size_of::<Document>() as u64 + 16),
        document.measured(Allocation::ReplaySafe),
    ])
}

/// Installation selects a power-of-two baseline shape. The live bound admits
/// both original capacities and this replay shape, so a save cannot lose room
/// when decoded Vec capacities differ. No normalisation runs on a warm edit.
pub(super) fn compact_baseline(document: &Document, limit: u64) -> Result<Document, Error> {
    super::metrics::exact(0);
    let bytes = document
        .measured(Allocation::Canonical)?
        .checked_add(size_of::<Document>() as u64 + 16)
        .ok_or(Error::Capacity)?;
    if bytes > limit {
        return Err(Error::DocumentCapacity);
    }
    super::metrics::document_copy();
    // This copy becomes the installed baseline, not a discarded estimator.
    Ok(document.copied(Allocation::Canonical))
}

/// Construction preserves all supplied spare capacity and pads undersized
/// buffers to the replay shape. This document becomes the retained baseline.
pub(super) fn padded_baseline(document: &Document) -> Document {
    super::metrics::document_copy();
    document.copied(Allocation::ReplaySafe)
}
