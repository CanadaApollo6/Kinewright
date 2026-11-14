//! Typed capacity bounds and the single candidate copy that preserves capacity.
use crate::undo::Error;
use crate::{
    AssetId, AudioBus, AudioBusId, AudioMaster, AudioMix, AutomationCurve, BinId, BlendMode, Clip,
    ClipContent, ClipId, ColorContext, ColorDescription, Document, Effect, EffectId, FreezeFrame,
    InvestigatorPreferences, Keyframe, LinkId, LutAsset, LutAssetId, LutAssetKind, LutAssetSource,
    Marker, MarkerId, MediaAsset, MediaBin, MediaCatalog, MediaKind, MediaSourceFingerprint,
    Operation, PanLaw, ParamValue, Rational, RelinkCandidate, SolidColor, SourceSelect, StringOut,
    StringOutId, SyncGroup, SyncGroupId, SyncGroupMember, TimeCode, Title, Track, TrackId,
    TrackKind, TrackMix, Transition,
};
use std::{collections::BTreeMap, mem::size_of, path::PathBuf};
pub(crate) trait Heap {
    fn heap(&self) -> Result<u64, Error>;
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
    fn preserving_copy(&self) -> Self;
}
fn inline_copy<T: Clone>(value: &T) -> T {
    value.clone()
}
macro_rules! inline {($($t:ty),*$(,)?)=>{$(
impl Heap for $t {fn heap(&self)->Result<u64,Error>{const {assert!(!std::mem::needs_drop::<$t>(),"owned field must be charged");} Ok(0)}}
impl Preserve for $t {fn preserving_copy(&self)->Self{inline_copy(self)}}
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
    crate::title::TitlePosition,
    crate::CaptionPreset,
    crate::automation::KeyframeInterpolation,
    crate::editorial::ThreePointMode
);
impl<T: Heap> Heap for Option<T> {
    fn heap(&self) -> Result<u64, Error> {
        self.as_ref().map_or(Ok(0), Heap::heap)
    }
}
impl<T: Heap> Heap for Vec<T> {
    fn heap(&self) -> Result<u64, Error> {
        sum([
            allocation::<T>(self.capacity())?,
            u64::from(self.capacity() > 0) * 16,
        ]
        .into_iter()
        .map(Ok)
        .chain(self.iter().map(Heap::heap)))
    }
}
impl Heap for String {
    fn heap(&self) -> Result<u64, Error> {
        Ok(self.capacity() as u64 + u64::from(self.capacity() > 0) * 16)
    }
}
impl Heap for PathBuf {
    fn heap(&self) -> Result<u64, Error> {
        Ok(self.capacity() as u64 + u64::from(self.capacity() > 0) * 16)
    }
}
impl<A: Heap, B: Heap> Heap for (A, B) {
    fn heap(&self) -> Result<u64, Error> {
        sum([self.0.heap(), self.1.heap()])
    }
}
impl<T: Heap, const N: usize> Heap for [T; N] {
    fn heap(&self) -> Result<u64, Error> {
        sum(self.iter().map(Heap::heap))
    }
}
impl<T: Heap> Heap for std::ops::Range<T> {
    fn heap(&self) -> Result<u64, Error> {
        sum([self.start.heap(), self.end.heap()])
    }
}
impl<K: Heap, V: Heap> Heap for BTreeMap<K, V> {
    fn heap(&self) -> Result<u64, Error> {
        // A node has at most eleven entries and twelve child links. One node per
        // entry plus an empty root is conservative even after removals.
        let node = allocation::<(K, V)>(11)?
            .checked_add(128)
            .ok_or(Error::Capacity)?;
        let nodes = (self.len() as u64 + 1)
            .checked_mul(node)
            .ok_or(Error::Capacity)?;
        sum(std::iter::once(Ok(nodes)).chain(self.iter().flat_map(|(k, v)| [k.heap(), v.heap()])))
    }
}
impl<T: Preserve> Preserve for Option<T> {
    fn preserving_copy(&self) -> Self {
        self.as_ref().map(Preserve::preserving_copy)
    }
}
impl<T: Preserve> Preserve for Vec<T> {
    fn preserving_copy(&self) -> Self {
        let mut result = Vec::with_capacity(self.capacity());
        result.extend(self.iter().map(Preserve::preserving_copy));
        result
    }
}
impl Preserve for String {
    fn preserving_copy(&self) -> Self {
        let mut result = Self::with_capacity(self.capacity());
        result.push_str(self);
        result
    }
}
impl Preserve for PathBuf {
    fn preserving_copy(&self) -> Self {
        let mut result = std::ffi::OsString::with_capacity(self.capacity());
        result.push(self.as_os_str());
        Self::from(result)
    }
}
impl<A: Preserve, B: Preserve> Preserve for (A, B) {
    fn preserving_copy(&self) -> Self {
        (self.0.preserving_copy(), self.1.preserving_copy())
    }
}
impl<T: Preserve, const N: usize> Preserve for [T; N] {
    fn preserving_copy(&self) -> Self {
        std::array::from_fn(|i| self[i].preserving_copy())
    }
}
impl<T: Preserve> Preserve for std::ops::Range<T> {
    fn preserving_copy(&self) -> Self {
        self.start.preserving_copy()..self.end.preserving_copy()
    }
}
impl<K: Preserve + Ord, V: Preserve> Preserve for BTreeMap<K, V> {
    fn preserving_copy(&self) -> Self {
        self.iter()
            .map(|(k, v)| (k.preserving_copy(), v.preserving_copy()))
            .collect()
    }
}
macro_rules! fields{($t:ident{$($f:ident),*$(,)?})=>{
impl Heap for $t{fn heap(&self)->Result<u64,Error>{let Self{$($f),*}=self;sum([$($f.heap()),*])}}
impl Preserve for $t{fn preserving_copy(&self)->Self{let Self{$($f),*}=self;Self{$($f:$f.preserving_copy()),*}}}
};}
fields!(MediaAsset {
    id,
    path,
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
    fps,
    resolution,
    duration
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
    fn heap(&self) -> Result<u64, Error> {
        match self {
            Self::Text(v) => v.heap(),
            Self::Integer(_) | Self::Boolean(_) => Ok(0),
        }
    }
}
impl Heap for ClipContent {
    fn heap(&self) -> Result<u64, Error> {
        match self {
            Self::Title(v) => v.heap(),
            Self::Media | Self::Freeze(_) | Self::Adjustment | Self::Solid(_) => Ok(0),
        }
    }
}
impl Heap for LutAssetSource {
    fn heap(&self) -> Result<u64, Error> {
        match self {
            Self::Imported { source_path } => source_path.heap(),
            Self::Builtin { name } => name.heap(),
        }
    }
}
impl Preserve for ParamValue {
    fn preserving_copy(&self) -> Self {
        match self {
            Self::Text(v) => Self::Text(v.preserving_copy()),
            Self::Integer(v) => Self::Integer(*v),
            Self::Boolean(v) => Self::Boolean(*v),
        }
    }
}
impl Preserve for ClipContent {
    fn preserving_copy(&self) -> Self {
        match self {
            Self::Media => Self::Media,
            Self::Adjustment => Self::Adjustment,
            Self::Title(v) => Self::Title(v.preserving_copy()),
            Self::Freeze(v) => Self::Freeze(v.preserving_copy()),
            Self::Solid(v) => Self::Solid(v.preserving_copy()),
        }
    }
}
impl Preserve for LutAssetSource {
    fn preserving_copy(&self) -> Self {
        match self {
            Self::Imported { source_path } => Self::Imported {
                source_path: source_path.preserving_copy(),
            },
            Self::Builtin { name } => Self::Builtin {
                name: name.preserving_copy(),
            },
        }
    }
}
impl Heap for Operation {
    #[allow(
        clippy::too_many_lines,
        reason = "every operation declares all owned fields in one exhaustive table"
    )]
    fn heap(&self) -> Result<u64, Error> {
        match self {
            Self::AddAsset { asset } => sum([asset.heap()]),
            Self::RelinkAsset {
                asset,
                candidate,
                allow_unverified_source,
            } => sum([
                asset.heap(),
                candidate.heap(),
                allow_unverified_source.heap(),
            ]),
            Self::SetAssetColorDescription {
                asset,
                color_description,
            } => sum([asset.heap(), color_description.heap()]),
            Self::SetColorContext { color_context } => sum([color_context.heap()]),
            Self::UpsertBin { bin } => sum([bin.heap()]),
            Self::RemoveBin { bin } => sum([bin.heap()]),
            Self::SetAssetBin { asset, bin } => sum([asset.heap(), bin.heap()]),
            Self::UpsertStringOut { string_out } => sum([string_out.heap()]),
            Self::RemoveStringOut { string_out } => sum([string_out.heap()]),
            Self::UpsertSyncGroup { sync_group } => sum([sync_group.heap()]),
            Self::RemoveSyncGroup { sync_group } => sum([sync_group.heap()]),
            Self::UpsertAudioBus { bus } => sum([bus.heap()]),
            Self::RemoveAudioBus { bus } => sum([bus.heap()]),
            Self::SetAudioMaster { master } => sum([master.heap()]),
            Self::SetPanLaw { law } => sum([law.heap()]),
            Self::AddTrack { track } => sum([track.heap()]),
            Self::RemoveTrack { track } => sum([track.heap()]),
            Self::SetTrackSyncLock { track, locked } => sum([track.heap(), locked.heap()]),
            Self::SetTrackMix {
                track,
                gain_tenth_db,
                pan_percent,
                mute,
                solo,
            } => sum([
                track.heap(),
                gain_tenth_db.heap(),
                pan_percent.heap(),
                mute.heap(),
                solo.heap(),
            ]),
            Self::SetTrackAutomation {
                track,
                parameter,
                curve,
            } => sum([track.heap(), parameter.heap(), curve.heap()]),
            Self::AddClip {
                track,
                asset,
                at,
                source,
            } => sum([track.heap(), asset.heap(), at.heap(), source.heap()]),
            Self::AddTitle {
                track,
                at,
                duration,
                title,
            } => sum([track.heap(), at.heap(), duration.heap(), title.heap()]),
            Self::SplitClip { clip, at } => sum([clip.heap(), at.heap()]),
            Self::TrimClip { clip, new_source } => sum([clip.heap(), new_source.heap()]),
            Self::MoveClip { clip, to_track, to } => sum([clip.heap(), to_track.heap(), to.heap()]),
            Self::ThreePointEdit {
                track,
                asset,
                source_in,
                source_out,
                timeline_in,
                timeline_out,
                mode,
            } => sum([
                track.heap(),
                asset.heap(),
                source_in.heap(),
                source_out.heap(),
                timeline_in.heap(),
                timeline_out.heap(),
                mode.heap(),
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
                asset.heap(),
                source_in.heap(),
                source_out.heap(),
                timeline_in.heap(),
                timeline_out.heap(),
                mode.heap(),
                video_track.heap(),
                audio_track.heap(),
            ]),
            Self::SlipClip {
                clip,
                new_source_in,
            } => sum([clip.heap(), new_source_in.heap()]),
            Self::RollEdit {
                left_clip,
                right_clip,
                to,
            } => sum([left_clip.heap(), right_clip.heap(), to.heap()]),
            Self::SlideClip { clip, to } => sum([clip.heap(), to.heap()]),
            Self::ReplaceClip {
                clip,
                asset,
                source,
            }
            | Self::FitToFill {
                clip,
                asset,
                source,
            } => sum([clip.heap(), asset.heap(), source.heap()]),
            Self::DeleteClip { clip }
            | Self::RippleDeleteClip { clip }
            | Self::RemoveTransition { clip } => sum([clip.heap()]),
            Self::RippleInsertGap {
                track,
                at,
                duration,
            } => sum([track.heap(), at.heap(), duration.heap()]),
            Self::LinkClips { clips } | Self::UnlinkClips { clips } => sum([clips.heap()]),
            Self::AddMarker { marker } => sum([marker.heap()]),
            Self::RemoveMarker { marker } => sum([marker.heap()]),
            Self::MoveMarker { marker, to } => sum([marker.heap(), to.heap()]),
            Self::AddEffect { clip, effect } => sum([clip.heap(), effect.heap()]),
            Self::InsertEffect {
                clip,
                index,
                effect,
            } => sum([clip.heap(), index.heap(), effect.heap()]),
            Self::RemoveEffect { clip, effect } => sum([clip.heap(), effect.heap()]),
            Self::SetEffectParam {
                clip,
                effect,
                name,
                value,
            } => sum([clip.heap(), effect.heap(), name.heap(), value.heap()]),
            Self::SetEffectKeyframes {
                clip,
                effect,
                name,
                curve,
            } => sum([clip.heap(), effect.heap(), name.heap(), curve.heap()]),
            Self::ClearEffectKeyframes { clip, effect, name } => {
                sum([clip.heap(), effect.heap(), name.heap()])
            }
            Self::UpsertEffectKeyframe {
                clip,
                effect,
                name,
                key,
            } => sum([clip.heap(), effect.heap(), name.heap(), key.heap()]),
            Self::RemoveEffectKeyframe {
                clip,
                effect,
                name,
                at,
            } => sum([clip.heap(), effect.heap(), name.heap(), at.heap()]),
            Self::SetEffectEnabled {
                clip,
                effect,
                enabled,
            } => sum([clip.heap(), effect.heap(), enabled.heap()]),
            Self::SetClipEnabled { clip, enabled } => sum([clip.heap(), enabled.heap()]),
            Self::SetClipEnabledCurve { clip, curve }
            | Self::SetClipGainEnvelope { clip, curve } => sum([clip.heap(), curve.heap()]),
            Self::CopyClipAttributes {
                from_clip,
                to_clip,
                names,
                include_keyframes,
            } => sum([
                from_clip.heap(),
                to_clip.heap(),
                names.heap(),
                include_keyframes.heap(),
            ]),
            Self::ConvertLegacyLook {
                clip,
                effect,
                lut_asset,
                mix_basis_points,
            } => sum([
                clip.heap(),
                effect.heap(),
                lut_asset.heap(),
                mix_basis_points.heap(),
            ]),
            Self::AddLutAsset { asset } => sum([asset.heap()]),
            Self::RemoveLutAsset { lut_asset } => sum([lut_asset.heap()]),
            Self::SetTitleParam { clip, name, value } => {
                sum([clip.heap(), name.heap(), value.heap()])
            }
            Self::SetClipAudio {
                clip,
                gain_tenth_db,
                fade_in_frames,
                fade_out_frames,
            } => sum([
                clip.heap(),
                gain_tenth_db.heap(),
                fade_in_frames.heap(),
                fade_out_frames.heap(),
            ]),
            Self::AddTransition { clip, transition } => sum([clip.heap(), transition.heap()]),
            Self::SetMarkerParam {
                marker,
                name,
                value,
            } => sum([marker.heap(), name.heap(), value.heap()]),
            Self::AddFreezeFrame {
                track,
                at,
                duration,
                asset,
                source_frame,
            } => sum([
                track.heap(),
                at.heap(),
                duration.heap(),
                asset.heap(),
                source_frame.heap(),
            ]),
            Self::SetClipSpeed {
                clip,
                speed_percent,
            } => sum([clip.heap(), speed_percent.heap()]),
            Self::SetClipBlendMode { clip, blend_mode } => sum([clip.heap(), blend_mode.heap()]),
            Self::AddAdjustmentClip {
                track,
                timeline_start,
                duration,
                effects,
            } => sum([
                track.heap(),
                timeline_start.heap(),
                duration.heap(),
                effects.heap(),
            ]),
            Self::AddSolidClip {
                track,
                timeline_start,
                duration,
                color,
            } => sum([
                track.heap(),
                timeline_start.heap(),
                duration.heap(),
                color.heap(),
            ]),
            Self::SetSolidColor { clip, color } => sum([clip.heap(), color.heap()]),
        }
    }
}
pub(crate) fn document_bytes(document: &Document) -> Result<u64, Error> {
    sum([Ok(size_of::<Document>() as u64 + 16), document.heap()])
}

/// Atomic candidate copy. Preserve capacities so in-place validated replay has
/// the same allocation bounds as admission, without a normalization copy.
pub(super) fn copy_document(document: &Document) -> Document {
    super::metrics::document_copy();
    document.preserving_copy()
}
