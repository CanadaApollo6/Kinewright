//! Pure operation policy. This is scope validation, not an H3 authorization grant.
//! Original Kinewright implementation; no upstream source was translated.
//!
//! Scope strings are exact, with no wildcards. Objects use `track:1`, `clip:2`,
//! `effect:2:3` (clip then effect), `asset:4`, `bus:5`, `mix:1`, `marker:6`,
//! `bin:7`, `string_out:8`, `sync_group:9`, `lut:10`, or `document`. Tracks use
//! `track:1`; the routing closure uses `track:1` and `bus:5`. Fields use model
//! names such as `clip.audio_gain_tenth_db`, `track.clips`, `mix.gain_tenth_db`,
//! `effect.parameters.percent`, and `effect.keyframes.percent`. Collection
//! membership is a field too, for example `document.tracks` or `document.buses`.
//! Creation/deletion requires the object's persisted fields and its membership
//! field. Generated clip/effect identities must be explicitly admitted.

use std::collections::{BTreeMap, BTreeSet};

use kinewright_core::{
    AudioBusId, Clip, ClipId, ColorNodeKind, Document, Effect, EffectId, Operation, Track, TrackId,
    TrackKind, apply_batch, effect_compatibility_stage, effect_descriptor, is_audio_effect,
};
use serde::Serialize;
use serde_json::{Map, Value};

use super::orchestration_records::{BranchBinding, Role, TaskScope};

#[derive(Debug, Clone)]
pub struct TaskPolicy {
    pub role: Role,
    pub scope: TaskScope,
}

macro_rules! operation_names {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        const OPERATION_NAMES: &[&str] = &[$($name),+];

        /// Return the compact schema name, exhaustively covering Core operations.
        #[must_use]
        pub fn operation_name(operation: &Operation) -> &'static str {
            match operation { $(Operation::$variant { .. } => $name),+ }
        }
    };
}

operation_names! {
    AddAsset => "add_asset", RelinkAsset => "relink_asset",
    SetAssetColorDescription => "set_asset_color_description",
    SetColorContext => "set_color_context", UpsertBin => "upsert_bin",
    RemoveBin => "remove_bin", SetAssetBin => "set_asset_bin",
    UpsertStringOut => "upsert_string_out", RemoveStringOut => "remove_string_out",
    UpsertSyncGroup => "upsert_sync_group", RemoveSyncGroup => "remove_sync_group",
    UpsertAudioBus => "upsert_audio_bus", RemoveAudioBus => "remove_audio_bus",
    SetAudioMaster => "set_audio_master", SetPanLaw => "set_pan_law",
    AddTrack => "add_track", RemoveTrack => "remove_track",
    SetTrackSyncLock => "set_track_sync_lock", SetTrackMix => "set_track_mix",
    SetTrackAutomation => "set_track_automation", AddClip => "add_clip",
    AddTitle => "add_title", SplitClip => "split_clip", TrimClip => "trim_clip",
    MoveClip => "move_clip", ThreePointEdit => "three_point_edit",
    PatchedThreePointEdit => "patched_three_point_edit", SlipClip => "slip_clip",
    RollEdit => "roll_edit", SlideClip => "slide_clip", ReplaceClip => "replace_clip",
    FitToFill => "fit_to_fill", DeleteClip => "delete_clip",
    RippleDeleteClip => "ripple_delete_clip", RippleInsertGap => "ripple_insert_gap",
    LinkClips => "link_clips", UnlinkClips => "unlink_clips", AddMarker => "add_marker",
    RemoveMarker => "remove_marker", MoveMarker => "move_marker",
    AddEffect => "add_effect", InsertEffect => "insert_effect",
    RemoveEffect => "remove_effect", SetEffectParam => "set_effect_param",
    SetEffectKeyframes => "set_effect_keyframes", ClearEffectKeyframes => "clear_effect_keyframes",
    UpsertEffectKeyframe => "upsert_effect_keyframe", RemoveEffectKeyframe => "remove_effect_keyframe",
    SetEffectEnabled => "set_effect_enabled", SetClipEnabled => "set_clip_enabled",
    SetClipEnabledCurve => "set_clip_enabled_curve", CopyClipAttributes => "copy_clip_attributes",
    ConvertLegacyLook => "convert_legacy_look", AddLutAsset => "add_lut_asset",
    RemoveLutAsset => "remove_lut_asset", SetTitleParam => "set_title_param",
    SetClipAudio => "set_clip_audio", SetClipGainEnvelope => "set_clip_gain_envelope",
    AddTransition => "add_transition", RemoveTransition => "remove_transition",
    SetMarkerParam => "set_marker_param", AddFreezeFrame => "add_freeze_frame",
    SetClipSpeed => "set_clip_speed", SetClipBlendMode => "set_clip_blend_mode",
    AddAdjustmentClip => "add_adjustment_clip", AddSolidClip => "add_solid_clip",
    SetSolidColor => "set_solid_color",
}

#[derive(Clone, Copy)]
enum Domain {
    Editorial,
    Motion,
    Audio,
    Colour,
}

impl TaskPolicy {
    /// Validate every declared target and every intermediate Core effect on a
    /// private document. The supplied document is never mutated, even on success.
    /// Callers must repeat this check at commit/replay/merge against the bound cut.
    /// Base identity, credentials, expiry, and input leases remain host checks.
    ///
    /// # Errors
    /// Refuses empty batches, unknown scope vocabulary, role/scope escapes,
    /// unsupported input acquisition, and invalid Core operations.
    pub fn validate_batch(
        &self,
        document: &Document,
        operations: &[Operation],
    ) -> Result<(), String> {
        self.validate_scope()?;
        if operations.is_empty() {
            return Err("empty operation batch".into());
        }
        if self.role == Role::Reviewer {
            return Err("reviewer is mutation-free".into());
        }
        let mut private = document.clone();
        for (index, operation) in operations.iter().enumerate() {
            let mut validate = || -> Result<(), String> {
                if !self.scope.operations.contains(operation_name(operation)) {
                    return Err(format!(
                        "operation outside scope: {}",
                        operation_name(operation)
                    ));
                }
                let mut tracks = BTreeSet::new();
                self.validate_operation(&private, operation, &mut tracks)?;
                let before = private.clone();
                apply_batch(&mut private, std::slice::from_ref(operation))
                    .map_err(|error| error.to_string())?;
                self.validate_changes(&before, &private, &mut tracks)?;
                let audio_operation = matches!(
                    operation,
                    Operation::UpsertAudioBus { .. }
                        | Operation::RemoveAudioBus { .. }
                        | Operation::SetAudioMaster { .. }
                        | Operation::SetPanLaw { .. }
                        | Operation::SetTrackMix { .. }
                        | Operation::SetTrackAutomation { .. }
                        | Operation::SetClipAudio { .. }
                        | Operation::SetClipGainEnvelope { .. }
                );
                if audio_operation || audio_changed(&before, &private) {
                    self.validate_routing(&before, &private, tracks)?;
                }
                Ok(())
            };
            validate().map_err(|error| {
                format!(
                    "operation {} ({}): {error}",
                    index + 1,
                    operation_name(operation)
                )
            })?;
        }
        Ok(())
    }

    fn validate_scope(&self) -> Result<(), String> {
        for operation in &self.scope.operations {
            if !OPERATION_NAMES.contains(&operation.as_str()) {
                return Err(format!("unknown operation scope: {operation}"));
            }
        }
        for object in &self.scope.objects {
            if !known_object(object) {
                return Err(format!("unknown object scope: {object}"));
            }
        }
        for field in &self.scope.fields {
            if !known_field(field) {
                return Err(format!("unknown field scope: {field}"));
            }
        }
        for track in &self.scope.tracks {
            if !numbered(track, "track") {
                return Err(format!("unknown track scope: {track}"));
            }
        }
        for member in &self.scope.bus_sidechain_closure {
            if !numbered(member, "track") && !numbered(member, "bus") {
                return Err(format!("unknown routing scope: {member}"));
            }
        }
        Ok(())
    }

    fn allow(&self, domain: Domain) -> Result<(), String> {
        let allowed = match self.role {
            Role::Director => true,
            Role::Editor => matches!(domain, Domain::Editorial),
            Role::Motion => matches!(domain, Domain::Motion),
            Role::Audio => matches!(domain, Domain::Audio),
            Role::Colour => matches!(domain, Domain::Colour),
            Role::Reviewer => false,
        };
        if allowed {
            Ok(())
        } else {
            Err("operation outside role domain".into())
        }
    }

    fn object(&self, object: &str) -> Result<(), String> {
        if self.scope.objects.contains(object) {
            Ok(())
        } else {
            Err(format!("object outside scope: {object}"))
        }
    }

    fn fields(&self, fields: &[&str]) -> Result<(), String> {
        for field in fields {
            if !known_field(field) || !self.scope.fields.contains(*field) {
                return Err(format!("field outside scope: {field}"));
            }
        }
        Ok(())
    }

    fn track(
        &self,
        document: &Document,
        id: TrackId,
        tracks: &mut BTreeSet<TrackId>,
    ) -> Result<(), String> {
        let track = document
            .tracks
            .iter()
            .find(|track| track.id == id)
            .ok_or_else(|| format!("missing track: {id}"))?;
        self.track_value(track, tracks)
    }

    fn track_value(&self, track: &Track, tracks: &mut BTreeSet<TrackId>) -> Result<(), String> {
        let key = format!("track:{}", track.id);
        self.object(&key)?;
        if !self.scope.tracks.contains(&key) {
            return Err(format!("track outside scope: {key}"));
        }
        let kind_allowed = match self.role {
            Role::Audio => track.kind == TrackKind::Audio,
            Role::Motion | Role::Colour => track.kind == TrackKind::Video,
            Role::Director | Role::Editor => true,
            Role::Reviewer => false,
        };
        if !kind_allowed {
            return Err(format!("track kind outside role: {key}"));
        }
        tracks.insert(track.id);
        Ok(())
    }

    fn track_clips(&self, document: &Document, id: TrackId) -> Result<(), String> {
        if let Some(track) = document.tracks.iter().find(|track| track.id == id) {
            for clip in &track.clips {
                self.object(&format!("clip:{}", clip.id))?;
            }
        }
        Ok(())
    }

    fn clip<'a>(
        &self,
        document: &'a Document,
        id: ClipId,
        tracks: &mut BTreeSet<TrackId>,
    ) -> Result<&'a Clip, String> {
        self.object(&format!("clip:{id}"))?;
        let track = document
            .tracks
            .iter()
            .find(|track| track.clips.iter().any(|clip| clip.id == id))
            .ok_or_else(|| format!("missing clip: {id}"))?;
        self.track_value(track, tracks)?;
        if document
            .clip(id)
            .is_some_and(|clip| matches!(clip.content, kinewright_core::ClipContent::Adjustment))
        {
            self.adjustment_targets(document, track.id, tracks)?;
        }
        document
            .clip(id)
            .ok_or_else(|| format!("missing clip: {id}"))
    }

    fn adjustment_targets(
        &self,
        document: &Document,
        owner: TrackId,
        tracks: &mut BTreeSet<TrackId>,
    ) -> Result<(), String> {
        for track in document.tracks.iter().take_while(|track| track.id != owner) {
            if track.kind == TrackKind::Video {
                self.track_value(track, tracks)?;
                for clip in &track.clips {
                    self.object(&format!("clip:{}", clip.id))?;
                }
            }
        }
        Ok(())
    }

    fn effect<'a>(
        &self,
        document: &'a Document,
        clip: ClipId,
        id: EffectId,
        tracks: &mut BTreeSet<TrackId>,
    ) -> Result<&'a Effect, String> {
        self.object(&format!("effect:{clip}:{id}"))?;
        let effect = self
            .clip(document, clip, tracks)?
            .effects
            .iter()
            .find(|effect| effect.id == id)
            .ok_or_else(|| format!("missing effect: {clip}:{id}"))?;
        self.allow(effect_domain(&effect.name)?)?;
        Ok(effect)
    }

    fn effect_value(&self, effect: &Effect) -> Result<(), String> {
        self.allow(effect_domain(&effect.name)?)?;
        self.effect_contents(effect)
    }

    fn effect_contents(&self, effect: &Effect) -> Result<(), String> {
        effect_domain(&effect.name)?;
        if effect.name == "cube_lut" {
            return Err("file-backed cube_lut requires input authority".into());
        }
        for name in effect.parameters.keys().chain(effect.keyframes.keys()) {
            Self::effect_parameter(effect, name)?;
        }
        if let Some(kinewright_core::ParamValue::Integer(id)) =
            effect.parameters.get("lut_asset_id")
        {
            self.object(&format!("lut:{id}"))?;
        }
        if let Some(curve) = effect.keyframes.get("lut_asset_id") {
            for key in &curve.keyframes {
                self.object(&format!("lut:{}", key.value))?;
            }
        }
        Ok(())
    }

    fn effect_parameter(effect: &Effect, name: &str) -> Result<(), String> {
        if name != "enabled"
            && effect_descriptor(&effect.name)
                .and_then(|descriptor| descriptor.parameter(name))
                .is_none()
        {
            return Err(format!("unknown effect parameter: {}.{name}", effect.name));
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn validate_operation(
        &self,
        document: &Document,
        operation: &Operation,
        tracks: &mut BTreeSet<TrackId>,
    ) -> Result<(), String> {
        match operation {
            Operation::AddAsset { .. }
            | Operation::RelinkAsset { .. }
            | Operation::AddLutAsset { .. } => {
                return Err("import/relink requires host input authority".into());
            }
            Operation::SetAssetColorDescription { asset, .. } => {
                self.allow(Domain::Colour)?;
                self.object(&format!("asset:{asset}"))?;
                self.fields(&["asset.color_description", "asset.assumed_from"])?;
                for track in &document.tracks {
                    for clip in &track.clips {
                        if clip.content.references_asset() && clip.asset == *asset {
                            self.clip(document, clip.id, tracks)?;
                        }
                    }
                }
            }
            Operation::SetColorContext { .. } => {
                self.allow(Domain::Colour)?;
                self.object("document")?;
                self.fields(&["document.color_context"])?;
                for track in &document.tracks {
                    if track.kind == TrackKind::Video {
                        self.track_value(track, tracks)?;
                        for clip in &track.clips {
                            self.object(&format!("clip:{}", clip.id))?;
                        }
                    }
                }
            }
            Operation::UpsertBin { bin } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("bin:{}", bin.id))?;
                self.fields(&["bin.name", "bin.parent", "bin.assets"])?;
                if let Some(parent) = bin.parent {
                    self.object(&format!("bin:{parent}"))?;
                }
                for asset in &bin.assets {
                    self.object(&format!("asset:{asset}"))?;
                }
            }
            Operation::RemoveBin { bin } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("bin:{bin}"))?;
                self.fields(&["document.bins"])?;
            }
            Operation::SetAssetBin { asset, bin } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("asset:{asset}"))?;
                self.fields(&["bin.assets"])?;
                if let Some(bin) = bin {
                    self.object(&format!("bin:{bin}"))?;
                }
            }
            Operation::UpsertStringOut { string_out } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("string_out:{}", string_out.id))?;
                self.fields(&["string_out.name", "string_out.selects"])?;
                for select in &string_out.selects {
                    self.object(&format!("asset:{}", select.asset))?;
                }
            }
            Operation::RemoveStringOut { string_out } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("string_out:{string_out}"))?;
                self.fields(&["document.string_outs"])?;
            }
            Operation::UpsertSyncGroup { sync_group } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("sync_group:{}", sync_group.id))?;
                self.fields(&["sync_group.name", "sync_group.members"])?;
                for member in &sync_group.members {
                    self.object(&format!("asset:{}", member.asset))?;
                }
            }
            Operation::RemoveSyncGroup { sync_group } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("sync_group:{sync_group}"))?;
                self.fields(&["document.sync_groups"])?;
            }
            Operation::UpsertAudioBus { bus } => {
                self.allow(Domain::Audio)?;
                self.object(&format!("bus:{}", bus.id))?;
                self.fields(&[
                    "bus.name",
                    "bus.tracks",
                    "bus.gain_tenth_db",
                    "bus.gain_curve",
                    "bus.effects",
                    "bus.ducking_sidechain_tracks",
                ])?;
                for id in bus.tracks.iter().chain(&bus.ducking_sidechain_tracks) {
                    self.track(document, *id, tracks)?;
                }
                if let Some(old) = document.audio_mix.buses.iter().find(|old| old.id == bus.id) {
                    for id in old.tracks.iter().chain(&old.ducking_sidechain_tracks) {
                        self.track(document, *id, tracks)?;
                    }
                }
                for effect in &bus.effects {
                    self.effect_value(effect)?;
                }
                self.routing_member(&format!("bus:{}", bus.id))?;
            }
            Operation::RemoveAudioBus { bus } => {
                self.allow(Domain::Audio)?;
                self.object(&format!("bus:{bus}"))?;
                self.fields(&["document.buses"])?;
                let bus = document
                    .audio_mix
                    .buses
                    .iter()
                    .find(|value| value.id == *bus)
                    .ok_or_else(|| format!("missing bus: {bus}"))?;
                for id in bus.tracks.iter().chain(&bus.ducking_sidechain_tracks) {
                    self.track(document, *id, tracks)?;
                }
                for effect in &bus.effects {
                    self.allow(effect_domain(&effect.name)?)?;
                }
                self.routing_member(&format!("bus:{}", bus.id))?;
            }
            Operation::SetAudioMaster { master } => {
                self.allow(Domain::Audio)?;
                self.object("document")?;
                self.fields(&["document.master"])?;
                for effect in &master.effects {
                    self.effect_value(effect)?;
                }
                for track in &document.tracks {
                    self.track_value(track, tracks)?;
                    self.track_clips(document, track.id)?;
                }
            }
            Operation::SetPanLaw { .. } => {
                self.allow(Domain::Audio)?;
                self.object("document")?;
                self.fields(&["document.pan_law"])?;
                for track in &document.tracks {
                    self.track_value(track, tracks)?;
                    self.track_clips(document, track.id)?;
                }
            }
            Operation::AddTrack { track } => {
                self.allow(Domain::Editorial)?;
                self.track_value(track, tracks)?;
                self.fields(&["document.tracks"])?;
            }
            Operation::RemoveTrack { track } => {
                self.allow(Domain::Editorial)?;
                self.track(document, *track, tracks)?;
                self.fields(&["document.tracks"])?;
            }
            Operation::SetTrackSyncLock { track, .. } => {
                self.allow(Domain::Editorial)?;
                self.track(document, *track, tracks)?;
                self.fields(&["track.sync_lock"])?;
            }
            Operation::SetTrackMix { track, solo, .. } => {
                self.allow(Domain::Audio)?;
                self.track(document, *track, tracks)?;
                self.track_clips(document, *track)?;
                self.object(&format!("mix:{track}"))?;
                self.fields(&[
                    "mix.gain_tenth_db",
                    "mix.pan_percent",
                    "mix.mute",
                    "mix.solo",
                ])?;
                if *solo || document.track_mix(*track).solo {
                    for track in &document.tracks {
                        self.track_value(track, tracks)?;
                        self.track_clips(document, track.id)?;
                    }
                }
            }
            Operation::SetTrackAutomation {
                track, parameter, ..
            } => {
                self.allow(Domain::Audio)?;
                self.track(document, *track, tracks)?;
                self.track_clips(document, *track)?;
                self.object(&format!("mix:{track}"))?;
                let field = match parameter.as_str() {
                    "gain_tenth_db" => "mix.gain_curve",
                    "pan_percent" => "mix.pan_curve",
                    _ => return Err(format!("unknown track automation: {parameter}")),
                };
                self.fields(&[field])?;
            }
            Operation::AddClip { track, asset, .. }
            | Operation::ThreePointEdit { track, asset, .. }
            | Operation::AddFreezeFrame { track, asset, .. } => {
                self.allow(Domain::Editorial)?;
                self.track(document, *track, tracks)?;
                self.object(&format!("asset:{asset}"))?;
                self.fields(&["track.clips"])?;
            }
            Operation::PatchedThreePointEdit {
                asset,
                video_track,
                audio_track,
                ..
            } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("asset:{asset}"))?;
                if video_track.is_none() && audio_track.is_none() {
                    return Err("no source-patch route".into());
                }
                for track in video_track.iter().chain(audio_track) {
                    self.track(document, *track, tracks)?;
                }
                self.fields(&["track.clips"])?;
            }
            Operation::AddTitle { track, .. } | Operation::AddSolidClip { track, .. } => {
                self.allow(Domain::Motion)?;
                self.track(document, *track, tracks)?;
                self.fields(&["track.clips"])?;
            }
            Operation::AddAdjustmentClip { track, effects, .. } => {
                if effects.is_empty() {
                    return Err("adjustment has no classified effects".into());
                }
                self.track(document, *track, tracks)?;
                self.adjustment_targets(document, *track, tracks)?;
                self.fields(&["track.clips"])?;
                for effect in effects {
                    self.effect_value(effect)?;
                }
            }
            Operation::MoveClip { clip, to_track, .. } => {
                self.allow(Domain::Editorial)?;
                self.clip(document, *clip, tracks)?;
                self.track(document, *to_track, tracks)?;
                self.fields(&["clip.timeline_start", "track.clips"])?;
            }
            Operation::SplitClip { clip, .. }
            | Operation::DeleteClip { clip }
            | Operation::RippleDeleteClip { clip } => {
                self.allow(Domain::Editorial)?;
                self.clip(document, *clip, tracks)?;
                self.fields(&["track.clips"])?;
            }
            Operation::TrimClip { clip, .. } | Operation::SlipClip { clip, .. } => {
                self.allow(Domain::Editorial)?;
                self.clip(document, *clip, tracks)?;
                self.fields(&["clip.source_range"])?;
            }
            Operation::SlideClip { clip, .. } => {
                self.allow(Domain::Editorial)?;
                self.clip(document, *clip, tracks)?;
                self.fields(&["clip.timeline_start", "clip.source_range"])?;
            }
            Operation::RollEdit {
                left_clip,
                right_clip,
                ..
            } => {
                self.allow(Domain::Editorial)?;
                self.clip(document, *left_clip, tracks)?;
                self.clip(document, *right_clip, tracks)?;
                self.fields(&["clip.source_range", "clip.timeline_start"])?;
            }
            Operation::ReplaceClip { clip, asset, .. }
            | Operation::FitToFill { clip, asset, .. } => {
                self.allow(Domain::Editorial)?;
                self.clip(document, *clip, tracks)?;
                self.object(&format!("asset:{asset}"))?;
                self.fields(&["clip.asset", "clip.source_range"])?;
            }
            Operation::RippleInsertGap { track, .. } => {
                self.allow(Domain::Editorial)?;
                self.track(document, *track, tracks)?;
                self.fields(&["clip.timeline_start", "marker.position"])?;
            }
            Operation::LinkClips { clips } | Operation::UnlinkClips { clips } => {
                self.allow(Domain::Editorial)?;
                if clips.is_empty() {
                    return Err("empty clip selection".into());
                }
                for clip in clips {
                    self.clip(document, *clip, tracks)?;
                }
                self.fields(&["clip.link"])?;
            }
            Operation::AddMarker { marker } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("marker:{}", marker.id))?;
                self.fields(&["document.markers"])?;
            }
            Operation::RemoveMarker { marker } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("marker:{marker}"))?;
                self.fields(&["document.markers"])?;
            }
            Operation::MoveMarker { marker, .. } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("marker:{marker}"))?;
                self.fields(&["marker.position"])?;
            }
            Operation::SetMarkerParam { marker, name, .. } => {
                self.allow(Domain::Editorial)?;
                self.object(&format!("marker:{marker}"))?;
                self.fields(&[&format!("marker.{name}")])?;
            }
            Operation::AddEffect { clip, effect }
            | Operation::InsertEffect { clip, effect, .. } => {
                self.clip(document, *clip, tracks)?;
                self.object(&format!("effect:{clip}:{}", effect.id))?;
                self.effect_value(effect)?;
                self.fields(&["clip.effects"])?;
            }
            Operation::RemoveEffect { clip, effect } => {
                self.effect(document, *clip, *effect, tracks)?;
                self.fields(&["clip.effects"])?;
            }
            Operation::SetEffectParam {
                clip,
                effect,
                name,
                value,
            } => {
                let effect = self.effect(document, *clip, *effect, tracks)?;
                self.effect_value(effect)?;
                Self::effect_parameter(effect, name)?;
                self.fields(&[&format!("effect.parameters.{name}")])?;
                if name == "lut_asset_id" {
                    let kinewright_core::ParamValue::Integer(id) = value else {
                        return Err("invalid LUT identity".into());
                    };
                    self.object(&format!("lut:{id}"))?;
                }
            }
            Operation::SetEffectKeyframes {
                clip, effect, name, ..
            }
            | Operation::ClearEffectKeyframes { clip, effect, name }
            | Operation::UpsertEffectKeyframe {
                clip, effect, name, ..
            }
            | Operation::RemoveEffectKeyframe {
                clip, effect, name, ..
            } => {
                let effect = self.effect(document, *clip, *effect, tracks)?;
                self.effect_value(effect)?;
                Self::effect_parameter(effect, name)?;
                if name == "enabled" {
                    self.fields(&["effect.enabled", "effect.enabled_curve"])?;
                } else {
                    self.fields(&[
                        &format!("effect.keyframes.{name}"),
                        &format!("effect.parameters.{name}"),
                    ])?;
                }
            }
            Operation::SetEffectEnabled { clip, effect, .. } => {
                let effect = self.effect(document, *clip, *effect, tracks)?;
                self.effect_value(effect)?;
                self.fields(&["effect.enabled"])?;
            }
            Operation::SetClipEnabled { clip, .. }
            | Operation::SetClipEnabledCurve { clip, .. } => {
                self.allow(Domain::Motion)?;
                self.clip(document, *clip, tracks)?;
                self.fields(&[if matches!(operation, Operation::SetClipEnabled { .. }) {
                    "clip.enabled"
                } else {
                    "clip.enabled_curve"
                }])?;
            }
            Operation::CopyClipAttributes {
                from_clip,
                to_clip,
                names,
                include_keyframes,
            } => {
                let source = self.clip(document, *from_clip, tracks)?;
                let target = self.clip(document, *to_clip, tracks)?;
                self.fields(&["clip.effects"])?;
                if let Some(names) = names {
                    if names.is_empty() {
                        return Err("empty effect selection".into());
                    }
                    for name in names {
                        self.allow(effect_domain(name)?)?;
                    }
                }
                let mut selected = false;
                for effect in &source.effects {
                    if names
                        .as_ref()
                        .is_none_or(|names| names.contains(&effect.name))
                    {
                        selected = true;
                        self.effect(document, *from_clip, effect.id, tracks)?;
                        self.effect_value(effect)?;
                        self.fields(&["effect.name", "effect.enabled", "effect.enabled_curve"])?;
                        for name in effect.parameters.keys() {
                            self.fields(&[&format!("effect.parameters.{name}")])?;
                        }
                        if *include_keyframes {
                            for name in effect.keyframes.keys() {
                                self.fields(&[&format!("effect.keyframes.{name}")])?;
                            }
                        }
                        for existing in &target.effects {
                            if existing.name == effect.name {
                                self.effect(document, *to_clip, existing.id, tracks)?;
                            }
                        }
                    }
                }
                if !selected {
                    return Err("no classified source effects".into());
                }
            }
            Operation::ConvertLegacyLook {
                clip,
                effect,
                lut_asset,
                ..
            } => {
                self.allow(Domain::Colour)?;
                self.effect(document, *clip, *effect, tracks)?;
                self.object(&format!("lut:{lut_asset}"))?;
                self.fields(&[
                    "effect.name",
                    "effect.parameters.lut_asset_id",
                    "effect.parameters.mix_basis_points",
                ])?;
            }
            Operation::RemoveLutAsset { lut_asset } => {
                self.allow(Domain::Colour)?;
                self.object(&format!("lut:{lut_asset}"))?;
                self.fields(&["document.lut_assets"])?;
            }
            Operation::SetTitleParam { clip, .. } | Operation::SetSolidColor { clip, .. } => {
                self.allow(Domain::Motion)?;
                self.clip(document, *clip, tracks)?;
                self.fields(&["clip.content"])?;
            }
            Operation::SetClipAudio { clip, .. } => {
                self.allow(Domain::Audio)?;
                self.clip(document, *clip, tracks)?;
                self.fields(&[
                    "clip.audio_gain_tenth_db",
                    "clip.audio_fade_in_frames",
                    "clip.audio_fade_out_frames",
                ])?;
            }
            Operation::SetClipGainEnvelope { clip, .. } => {
                self.allow(Domain::Audio)?;
                self.clip(document, *clip, tracks)?;
                self.fields(&["clip.audio_gain_curve"])?;
            }
            Operation::AddTransition { clip, .. } | Operation::RemoveTransition { clip } => {
                self.allow(Domain::Motion)?;
                self.clip(document, *clip, tracks)?;
                self.fields(&["clip.transition_in"])?;
            }
            Operation::SetClipSpeed { clip, .. } => {
                self.allow(Domain::Editorial)?;
                self.clip(document, *clip, tracks)?;
                self.fields(&["clip.speed_percent"])?;
            }
            Operation::SetClipBlendMode { clip, .. } => {
                self.allow(Domain::Motion)?;
                self.clip(document, *clip, tracks)?;
                self.fields(&["clip.blend_mode"])?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines, reason = "one closed model footprint check")]
    fn validate_changes(
        &self,
        before: &Document,
        after: &Document,
        tracks: &mut BTreeSet<TrackId>,
    ) -> Result<(), String> {
        let old = document_objects(before)?;
        let new = document_objects(after)?;
        for key in old.keys().chain(new.keys()).collect::<BTreeSet<_>>() {
            let left = old.get(key);
            let right = new.get(key);
            if left == right {
                continue;
            }
            self.object(key)?;
            let kind = key.split(':').next().ok_or("missing object kind")?;
            let fields: BTreeSet<_> = left.into_iter().chain(right).flat_map(Map::keys).collect();
            for field in fields {
                if left.and_then(|value| value.get(field))
                    != right.and_then(|value| value.get(field))
                {
                    let field = format!("{kind}.{field}");
                    if self.role == Role::Audio && !audio_field(&field) {
                        return Err(format!("non-audio collateral: {field}"));
                    }
                    self.fields(&[&field])?;
                }
            }
            for (is_after, document) in [(false, before), (true, after)] {
                for track in &document.tracks {
                    if key == &format!("track:{}", track.id) || key == &format!("mix:{}", track.id)
                    {
                        self.track_value(track, tracks)?;
                    }
                    for clip in &track.clips {
                        if key == &format!("clip:{}", clip.id)
                            || key.starts_with(&format!("effect:{}:", clip.id))
                        {
                            self.clip(document, clip.id, tracks)?;
                            if clip.content.references_asset() {
                                self.object(&format!("asset:{}", clip.asset))?;
                            }
                        }
                        for effect in &clip.effects {
                            if key == &format!("effect:{}:{}", clip.id, effect.id) {
                                self.indirect_effect(effect, is_after)?;
                            }
                        }
                    }
                }
                for bus in &document.audio_mix.buses {
                    if key == &format!("bus:{}", bus.id) {
                        self.routing_member(key)?;
                        for id in bus.tracks.iter().chain(&bus.ducking_sidechain_tracks) {
                            self.track(document, *id, tracks)?;
                        }
                        for effect in &bus.effects {
                            self.indirect_effect(effect, is_after)?;
                        }
                    }
                }
                for bin in &document.catalog.bins {
                    if key == &format!("bin:{}", bin.id) {
                        if let Some(parent) = bin.parent {
                            self.object(&format!("bin:{parent}"))?;
                        }
                        for asset in &bin.assets {
                            self.object(&format!("asset:{asset}"))?;
                        }
                    }
                }
                for item in &document.catalog.string_outs {
                    if key == &format!("string_out:{}", item.id) {
                        for select in &item.selects {
                            self.object(&format!("asset:{}", select.asset))?;
                        }
                    }
                }
                for group in &document.catalog.sync_groups {
                    if key == &format!("sync_group:{}", group.id) {
                        for member in &group.members {
                            self.object(&format!("asset:{}", member.asset))?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // Direct operations have already passed validate_operation. An Editor's
    // remaining effect changes are Core's preservation/rebasing for that edit.
    fn indirect_effect(&self, effect: &Effect, is_after: bool) -> Result<(), String> {
        let domain = effect_domain(&effect.name)?;
        if self.role != Role::Editor {
            self.allow(domain)?;
        }
        if is_after {
            self.effect_contents(effect)?;
        }
        Ok(())
    }

    fn routing_member(&self, key: &str) -> Result<(), String> {
        self.object(key)?;
        if !self.scope.bus_sidechain_closure.contains(key) {
            return Err(format!("routing closure outside scope: {key}"));
        }
        Ok(())
    }

    fn validate_routing(
        &self,
        before: &Document,
        after: &Document,
        mut tracks: BTreeSet<TrackId>,
    ) -> Result<(), String> {
        let mut audibility = BTreeSet::new();
        for track in &after.tracks {
            if before.tracks.iter().any(|old| old.id == track.id)
                && before.track_audible(track.id) != after.track_audible(track.id)
            {
                audibility.insert(track.id);
            }
        }
        for id in &audibility {
            for document in [before, after] {
                self.track(document, *id, &mut tracks)?;
                self.track_clips(document, *id)?;
            }
        }
        let mut buses = BTreeSet::<AudioBusId>::new();
        let shared_master = !tracks.is_empty()
            && (!before.audio_mix.master.effects.is_empty()
                || !after.audio_mix.master.effects.is_empty());
        if shared_master {
            self.object("document")?;
            tracks.extend(
                before
                    .tracks
                    .iter()
                    .chain(&after.tracks)
                    .map(|track| track.id),
            );
        }
        loop {
            let previous = (tracks.len(), buses.len());
            for document in [before, after] {
                for bus in &document.audio_mix.buses {
                    if buses.contains(&bus.id)
                        || bus
                            .tracks
                            .iter()
                            .chain(&bus.ducking_sidechain_tracks)
                            .any(|id| tracks.contains(id))
                    {
                        buses.insert(bus.id);
                        tracks.extend(
                            bus.tracks
                                .iter()
                                .chain(&bus.ducking_sidechain_tracks)
                                .copied(),
                        );
                    }
                }
            }
            if previous == (tracks.len(), buses.len()) {
                break;
            }
        }
        for bus in &buses {
            self.routing_member(&format!("bus:{bus}"))?;
        }
        for id in &tracks {
            let key = format!("track:{id}");
            self.routing_member(&key)?;
            for document in [before, after] {
                if let Some(track) = document.tracks.iter().find(|track| track.id == *id) {
                    self.track_value(track, &mut BTreeSet::new())?;
                    if shared_master || !buses.is_empty() {
                        self.track_clips(document, track.id)?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn audio_clips(document: &Document) -> BTreeMap<ClipId, (TrackId, &Clip)> {
    document
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter().map(move |clip| (track.id, clip)))
        .filter(|(_, clip)| {
            clip.content.is_media()
                && document
                    .asset(clip.asset)
                    .is_some_and(|asset| asset.kind.supports(TrackKind::Audio))
        })
        .map(|(track, clip)| (clip.id, (track, clip)))
        .collect()
}

fn audio_changed(before: &Document, after: &Document) -> bool {
    if before.audio_mix != after.audio_mix || before.fps != after.fps {
        return true;
    }
    let old = audio_clips(before);
    let new = audio_clips(after);
    old.keys().chain(new.keys()).any(|id| {
        let (Some((old_track, left)), Some((new_track, right))) = (old.get(id), new.get(id)) else {
            return true;
        };
        old_track != new_track
            || left.asset != right.asset
            || left.source_range != right.source_range
            || left.timeline_start != right.timeline_start
            || left.transition_in != right.transition_in
            || left.audio_gain_tenth_db != right.audio_gain_tenth_db
            || left.audio_fade_in_frames != right.audio_fade_in_frames
            || left.audio_fade_out_frames != right.audio_fade_out_frames
            || left.audio_gain_curve != right.audio_gain_curve
            || left.speed_percent != right.speed_percent
            || left.enabled != right.enabled
            || left.enabled_curve != right.enabled_curve
    })
}

fn effect_domain(name: &str) -> Result<Domain, String> {
    if effect_descriptor(name).is_none() {
        return Err(format!("unregistered effect: {name}"));
    }
    if is_audio_effect(name) {
        return Ok(Domain::Audio);
    }
    if ColorNodeKind::from_effect_name(name).is_some() || effect_compatibility_stage(name).is_some()
    {
        return Ok(Domain::Colour);
    }
    match name {
        "opacity" | "transform" | "crop" | "reframe" | "mask" | "chroma_key" => Ok(Domain::Motion),
        "color_grade" => Ok(Domain::Colour),
        _ => Err(format!("unclassified effect: {name}")),
    }
}

fn canonical_id(id: &str) -> bool {
    id.parse::<u64>().is_ok_and(|value| value.to_string() == id)
}

fn numbered(key: &str, kind: &str) -> bool {
    key.strip_prefix(kind)
        .and_then(|tail| tail.strip_prefix(':'))
        .is_some_and(canonical_id)
}

fn known_object(key: &str) -> bool {
    key == "document"
        || [
            "track",
            "clip",
            "asset",
            "bus",
            "mix",
            "marker",
            "bin",
            "string_out",
            "sync_group",
            "lut",
        ]
        .iter()
        .any(|kind| numbered(key, kind))
        || key
            .strip_prefix("effect:")
            .and_then(|tail| tail.split_once(':'))
            .is_some_and(|(clip, effect)| canonical_id(clip) && canonical_id(effect))
}

const FIELDS: &[(&str, &[&str])] = &[
    (
        "document",
        &[
            "tracks",
            "media_pool",
            "markers",
            "bins",
            "string_outs",
            "sync_groups",
            "buses",
            "mixes",
            "master",
            "pan_law",
            "color_context",
            "lut_assets",
            "investigator",
            "fps",
            "resolution",
            "duration",
        ],
    ),
    ("track", &["id", "kind", "sync_lock", "clips"]),
    (
        "clip",
        &[
            "id",
            "asset",
            "source_range",
            "content",
            "timeline_start",
            "effects",
            "transition_in",
            "link",
            "audio_gain_tenth_db",
            "audio_fade_in_frames",
            "audio_fade_out_frames",
            "audio_gain_curve",
            "speed_percent",
            "enabled",
            "enabled_curve",
            "blend_mode",
        ],
    ),
    ("effect", &["id", "name", "enabled", "enabled_curve"]),
    (
        "asset",
        &[
            "id",
            "path",
            "name",
            "duration",
            "fps",
            "kind",
            "resolution",
            "source_fingerprint",
            "color_description",
            "assumed_from",
        ],
    ),
    ("marker", &["id", "position", "label", "color_token"]),
    ("bin", &["id", "name", "parent", "assets"]),
    ("string_out", &["id", "name", "selects"]),
    ("sync_group", &["id", "name", "members"]),
    (
        "bus",
        &[
            "id",
            "name",
            "tracks",
            "gain_tenth_db",
            "gain_curve",
            "effects",
            "ducking_sidechain_tracks",
        ],
    ),
    (
        "mix",
        &[
            "track",
            "gain_tenth_db",
            "pan_percent",
            "gain_curve",
            "pan_curve",
            "mute",
            "solo",
        ],
    ),
    (
        "lut",
        &[
            "id",
            "sha256",
            "title",
            "kind",
            "size",
            "byte_len",
            "domain_min_millionths",
            "domain_max_millionths",
            "source",
        ],
    ),
];

fn known_field(field: &str) -> bool {
    let Some((kind, name)) = field.split_once('.') else {
        return false;
    };
    if FIELDS
        .iter()
        .any(|(owner, fields)| *owner == kind && fields.contains(&name))
    {
        return true;
    }
    if kind != "effect" {
        return false;
    }
    let Some((container, parameter)) = name.split_once('.') else {
        return false;
    };
    (container == "parameters" && parameter == "path")
        || (matches!(container, "parameters" | "keyframes")
            && kinewright_core::EFFECT_DESCRIPTORS
                .iter()
                .any(|effect| effect.parameter(parameter).is_some()))
}

fn audio_field(field: &str) -> bool {
    field.starts_with("bus.")
        || field.starts_with("mix.")
        || matches!(
            field,
            "clip.audio_gain_tenth_db"
                | "clip.audio_fade_in_frames"
                | "clip.audio_fade_out_frames"
                | "clip.audio_gain_curve"
                | "document.buses"
                | "document.mixes"
                | "document.master"
                | "document.pan_law"
        )
}

type Objects = BTreeMap<String, Map<String, Value>>;

fn record(value: &impl Serialize) -> Result<Map<String, Value>, String> {
    let Value::Object(value) = serde_json::to_value(value).map_err(|error| error.to_string())?
    else {
        return Err("expected model object".into());
    };
    Ok(value)
}

fn put(objects: &mut Objects, key: String, value: Map<String, Value>) -> Result<(), String> {
    let kind = key.split(':').next().ok_or("missing model kind")?;
    for field in value.keys() {
        if !known_field(&format!("{kind}.{field}")) {
            return Err(format!("unclassified model field: {kind}.{field}"));
        }
    }
    if objects.insert(key, value).is_some() {
        return Err("duplicate model identity".into());
    }
    Ok(())
}

fn ids(values: impl Iterator<Item = u64>) -> Value {
    Value::Array(values.map(Value::from).collect())
}

#[allow(
    clippy::too_many_lines,
    reason = "explicit model ownership and closed field vocabulary"
)]
fn document_objects(document: &Document) -> Result<Objects, String> {
    let mut objects = Objects::new();
    let mut root = record(document)?;
    root.insert(
        "tracks".into(),
        ids(document.tracks.iter().map(|track| track.id.0)),
    );
    root.insert(
        "media_pool".into(),
        ids(document.media_pool.iter().map(|asset| asset.id.0)),
    );
    root.insert(
        "markers".into(),
        ids(document.markers.iter().map(|marker| marker.id.0)),
    );
    root.insert(
        "lut_assets".into(),
        ids(document.lut_assets.iter().map(|asset| asset.id.0)),
    );
    let catalog = record(&document.catalog)?;
    if catalog
        .keys()
        .any(|field| !["bins", "string_outs", "sync_groups"].contains(&field.as_str()))
    {
        return Err("unclassified catalog field".into());
    }
    root.remove("catalog");
    root.insert(
        "bins".into(),
        ids(document.catalog.bins.iter().map(|bin| bin.id.0)),
    );
    root.insert(
        "string_outs".into(),
        ids(document.catalog.string_outs.iter().map(|item| item.id.0)),
    );
    root.insert(
        "sync_groups".into(),
        ids(document.catalog.sync_groups.iter().map(|item| item.id.0)),
    );
    let mix = record(&document.audio_mix)?;
    if mix
        .keys()
        .any(|field| !["buses", "tracks", "master", "pan_law"].contains(&field.as_str()))
    {
        return Err("unclassified audio mix field".into());
    }
    root.remove("audio_mix");
    root.insert(
        "buses".into(),
        ids(document.audio_mix.buses.iter().map(|bus| bus.id.0)),
    );
    root.insert(
        "mixes".into(),
        ids(document.audio_mix.tracks.iter().map(|mix| mix.track.0)),
    );
    root.insert(
        "master".into(),
        serde_json::to_value(&document.audio_mix.master).map_err(|error| error.to_string())?,
    );
    root.insert(
        "pan_law".into(),
        serde_json::to_value(document.audio_mix.pan_law).map_err(|error| error.to_string())?,
    );
    put(&mut objects, "document".into(), root)?;
    for track in &document.tracks {
        let mut value = record(track)?;
        value.insert(
            "clips".into(),
            ids(track.clips.iter().map(|clip| clip.id.0)),
        );
        put(&mut objects, format!("track:{}", track.id), value)?;
        for clip in &track.clips {
            let mut value = record(clip)?;
            value.insert(
                "effects".into(),
                ids(clip.effects.iter().map(|effect| effect.id.0)),
            );
            put(&mut objects, format!("clip:{}", clip.id), value)?;
            for effect in &clip.effects {
                let mut value = record(effect)?;
                value.remove("parameters");
                value.remove("keyframes");
                for (name, parameter) in &effect.parameters {
                    value.insert(
                        format!("parameters.{name}"),
                        serde_json::to_value(parameter).map_err(|error| error.to_string())?,
                    );
                }
                for (name, curve) in &effect.keyframes {
                    value.insert(
                        format!("keyframes.{name}"),
                        serde_json::to_value(curve).map_err(|error| error.to_string())?,
                    );
                }
                put(
                    &mut objects,
                    format!("effect:{}:{}", clip.id, effect.id),
                    value,
                )?;
            }
        }
    }
    for asset in &document.media_pool {
        put(&mut objects, format!("asset:{}", asset.id), record(asset)?)?;
    }
    for marker in &document.markers {
        put(
            &mut objects,
            format!("marker:{}", marker.id),
            record(marker)?,
        )?;
    }
    for bin in &document.catalog.bins {
        put(&mut objects, format!("bin:{}", bin.id), record(bin)?)?;
    }
    for item in &document.catalog.string_outs {
        put(
            &mut objects,
            format!("string_out:{}", item.id),
            record(item)?,
        )?;
    }
    for item in &document.catalog.sync_groups {
        put(
            &mut objects,
            format!("sync_group:{}", item.id),
            record(item)?,
        )?;
    }
    for bus in &document.audio_mix.buses {
        put(&mut objects, format!("bus:{}", bus.id), record(bus)?)?;
    }
    for mix in &document.audio_mix.tracks {
        put(&mut objects, format!("mix:{}", mix.track), record(mix)?)?;
    }
    for lut in &document.lut_assets {
        put(&mut objects, format!("lut:{}", lut.id), record(lut)?)?;
    }
    Ok(objects)
}

/// The scope S4 admits for a Revise cut Director: every operation, every base
/// model field, and every object, track and routing member of the document at
/// `base`. Exact names only. Per-effect parameter fields are not included: the
/// whole vocabulary is larger than one scope list may name, so effect tuning needs
/// its own narrower task scope. Clip and effect identities created later are not
/// pre-admitted either.
///
/// # Errors
/// The document has a field outside the closed vocabulary, or more objects,
/// tracks or routing members than one scope may name.
pub fn director_scope(document: &Document, base: BranchBinding) -> Result<TaskScope, String> {
    let limit = super::orchestration_records::ITEM_LIMIT;
    let fields: BTreeSet<String> = FIELDS
        .iter()
        .flat_map(|(kind, fields)| fields.iter().map(move |field| format!("{kind}.{field}")))
        .filter(|field| known_field(field))
        .collect();
    let objects: BTreeSet<String> = document_objects(document)?.into_keys().collect();
    let tracks: BTreeSet<String> = document
        .tracks
        .iter()
        .map(|track| format!("track:{}", track.id))
        .collect();
    let mut closure = tracks.clone();
    closure.extend(
        document
            .audio_mix
            .buses
            .iter()
            .map(|bus| format!("bus:{}", bus.id)),
    );
    if objects.len() > limit || closure.len() > limit || fields.len() > limit {
        return Err(format!(
            "This project has more than {limit} clips, tracks and other parts, more than one revised-cut run can work on yet. Your notes are saved."
        ));
    }
    Ok(TaskScope {
        operations: OPERATION_NAMES.iter().map(|name| (*name).into()).collect(),
        objects,
        fields,
        tracks,
        bus_sidechain_closure: closure,
        asset_roots: vec![],
        base,
        consent_refs: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::super::orchestration_records::BranchBinding;
    use super::*;
    use kinewright_core::{
        AssetId, AudioBus, ColorDescription, MediaAsset, MediaKind, MediaSourceFingerprint,
        ParamValue, Rational, TimeCode,
    };
    use kinewright_project::bundle_records::{ObjectRef, RecordKind};

    fn fixture() -> Document {
        let mut document = Document::default();
        let track = |id, kind| Track {
            id: TrackId(id),
            kind,
            sync_lock: true,
            clips: vec![],
        };
        apply_batch(
            &mut document,
            &[
                Operation::AddAsset {
                    asset: MediaAsset {
                        id: AssetId(1),
                        path: "fixture.mp4".into(),
                        name: "fixture".into(),
                        duration: TimeCode(600),
                        fps: Rational::new(30, 1).unwrap(),
                        kind: MediaKind::AudioVideo,
                        resolution: Some((1920, 1080)),
                        source_fingerprint: MediaSourceFingerprint::default(),
                        color_description: ColorDescription::default(),
                        assumed_from: None,
                    },
                },
                Operation::AddTrack {
                    track: track(1, TrackKind::Video),
                },
                Operation::AddTrack {
                    track: track(2, TrackKind::Audio),
                },
                Operation::AddTrack {
                    track: track(3, TrackKind::Audio),
                },
                Operation::AddClip {
                    track: TrackId(1),
                    asset: AssetId(1),
                    at: TimeCode(120),
                    source: TimeCode(0)..TimeCode(60),
                },
                Operation::AddClip {
                    track: TrackId(2),
                    asset: AssetId(1),
                    at: TimeCode(0),
                    source: TimeCode(0)..TimeCode(60),
                },
                Operation::AddClip {
                    track: TrackId(3),
                    asset: AssetId(1),
                    at: TimeCode(120),
                    source: TimeCode(0)..TimeCode(60),
                },
            ],
        )
        .unwrap();
        document
    }

    fn base() -> BranchBinding {
        BranchBinding {
            branch: serde_json::from_value(Value::String(format!("{:032x}", 1))).unwrap(),
            head: serde_json::from_value(Value::String(format!("{:032x}", 2))).unwrap(),
            checkpoint: ObjectRef {
                sha256: "3".repeat(64),
                byte_length: 1,
                record_kind: RecordKind::BranchCheckpoint,
            },
            document_hash: "4".repeat(64),
        }
    }

    fn policy(document: &Document, role: Role) -> TaskPolicy {
        let mut fields: BTreeSet<String> = FIELDS
            .iter()
            .flat_map(|(kind, fields)| fields.iter().map(move |field| format!("{kind}.{field}")))
            .collect();
        for descriptor in kinewright_core::EFFECT_DESCRIPTORS {
            for parameter in descriptor.parameters {
                fields.insert(format!("effect.parameters.{}", parameter.name));
                fields.insert(format!("effect.keyframes.{}", parameter.name));
            }
        }
        fields.insert("effect.parameters.path".into());
        let mut objects: BTreeSet<_> = document_objects(document).unwrap().into_keys().collect();
        for clip in 1..=10 {
            objects.insert(format!("clip:{clip}"));
            for effect in 1..=4 {
                objects.insert(format!("effect:{clip}:{effect}"));
            }
        }
        objects.extend(["mix:1", "mix:2", "mix:3", "bus:1", "bus:2"].map(str::to_owned));
        TaskPolicy {
            role,
            scope: TaskScope {
                operations: OPERATION_NAMES.iter().map(|name| (*name).into()).collect(),
                objects,
                fields,
                tracks: ["track:1", "track:2", "track:3"].map(str::to_owned).into(),
                bus_sidechain_closure: ["track:1", "track:2", "track:3", "bus:1", "bus:2"]
                    .map(str::to_owned)
                    .into(),
                asset_roots: vec![],
                consent_refs: vec![],
                base: base(),
            },
        }
    }

    fn audio(clip: u64, gain: i32) -> Operation {
        Operation::SetClipAudio {
            clip: ClipId(clip),
            gain_tenth_db: gain,
            fade_in_frames: TimeCode(3),
            fade_out_frames: TimeCode(4),
        }
    }

    fn effect(name: &str, id: u64) -> Effect {
        Effect {
            id: EffectId(id),
            name: name.into(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
            enabled: true,
            enabled_curve: None,
        }
    }

    fn bus(id: u64, tracks: &[u64], sidechains: &[u64]) -> AudioBus {
        AudioBus {
            id: AudioBusId(id),
            name: format!("bus {id}"),
            tracks: tracks.iter().map(|id| TrackId(*id)).collect(),
            gain_tenth_db: 0,
            gain_curve: None,
            effects: vec![],
            ducking_sidechain_tracks: sidechains.iter().map(|id| TrackId(*id)).collect(),
        }
    }

    #[test]
    fn scoped_audio_batch_changes_shaping_without_mutating_the_validation_input() {
        let document = fixture();
        let original = document.clone();
        let policy = TaskPolicy {
            role: Role::Audio,
            scope: TaskScope {
                operations: ["set_clip_audio".into()].into(),
                objects: ["track:2", "clip:2", "asset:1"].map(str::to_owned).into(),
                fields: [
                    "clip.audio_gain_tenth_db",
                    "clip.audio_fade_in_frames",
                    "clip.audio_fade_out_frames",
                ]
                .map(str::to_owned)
                .into(),
                tracks: ["track:2".into()].into(),
                bus_sidechain_closure: ["track:2".into()].into(),
                asset_roots: vec![],
                consent_refs: vec![],
                base: base(),
            },
        };
        let operations = [audio(2, -90), audio(2, -120)];
        policy.validate_batch(&document, &operations).unwrap();
        assert_eq!(document, original);
        let mut applied = document.clone();
        apply_batch(&mut applied, &operations).unwrap();
        let clip = applied.clip(ClipId(2)).unwrap();
        assert_eq!(
            (
                clip.audio_gain_tenth_db,
                clip.audio_fade_in_frames,
                clip.audio_fade_out_frames
            ),
            (-120, TimeCode(3), TimeCode(4))
        );
        assert_eq!(applied.tracks[0], document.tracks[0]);
        assert_eq!(applied.color_context, document.color_context);
    }

    #[test]
    fn operation_object_field_and_track_permissions_are_independent() {
        let document = fixture();
        let allowed = policy(&document, Role::Audio);
        allowed.validate_batch(&document, &[audio(2, -40)]).unwrap();
        for missing in ["operation", "object", "field", "track", "closure"] {
            let mut narrowed = allowed.clone();
            match missing {
                "operation" => {
                    narrowed.scope.operations.remove("set_clip_audio");
                }
                "object" => {
                    narrowed.scope.objects.remove("clip:2");
                }
                "field" => {
                    narrowed.scope.fields.remove("clip.audio_gain_tenth_db");
                }
                "track" => {
                    narrowed.scope.tracks.remove("track:2");
                }
                "closure" => {
                    narrowed.scope.bus_sidechain_closure.remove("track:2");
                }
                _ => unreachable!(),
            }
            assert!(
                narrowed
                    .validate_batch(&document, &[audio(2, -40)])
                    .is_err(),
                "missing {missing}"
            );
        }
    }

    #[test]
    fn late_refusal_and_temporary_scope_escapes_leave_the_exact_document_unchanged() {
        let document = fixture();
        let original = document.clone();
        let policy = policy(&document, Role::Audio);
        assert!(
            policy
                .validate_batch(&document, &[audio(2, -80), audio(1, -80), audio(1, 0)])
                .is_err()
        );
        assert_eq!(document, original);
        let invalid_core_value = audio(2, -900);
        assert!(
            policy
                .validate_batch(&document, &[audio(2, -80), invalid_core_value])
                .is_err()
        );
        assert_eq!(document, original);
    }

    #[test]
    fn no_op_arguments_and_empty_selections_do_not_bypass_role_or_scope() {
        let document = fixture();
        let audio_policy = policy(&document, Role::Audio);
        assert!(audio_policy.validate_batch(&document, &[]).is_err());
        let video_no_op = Operation::SetClipAudio {
            clip: ClipId(1),
            gain_tenth_db: 0,
            fade_in_frames: TimeCode(0),
            fade_out_frames: TimeCode(0),
        };
        assert!(
            audio_policy
                .validate_batch(&document, &[video_no_op])
                .is_err()
        );
        let mut narrowed = audio_policy.clone();
        narrowed.scope.fields.remove("clip.audio_gain_curve");
        assert!(
            narrowed
                .validate_batch(
                    &document,
                    &[Operation::SetClipGainEnvelope {
                        clip: ClipId(2),
                        curve: None
                    }]
                )
                .is_err()
        );
        let editor = policy(&document, Role::Editor);
        assert!(
            editor
                .validate_batch(&document, &[Operation::UnlinkClips { clips: vec![] }])
                .is_err()
        );
        assert!(
            editor
                .validate_batch(
                    &document,
                    &[Operation::CopyClipAttributes {
                        from_clip: ClipId(1),
                        to_clip: ClipId(1),
                        names: Some(vec![]),
                        include_keyframes: false
                    }]
                )
                .is_err()
        );
        assert!(
            editor
                .validate_batch(
                    &document,
                    &[Operation::PatchedThreePointEdit {
                        asset: AssetId(1),
                        source_in: None,
                        source_out: None,
                        timeline_in: None,
                        timeline_out: None,
                        mode: kinewright_core::ThreePointMode::Overwrite,
                        video_track: None,
                        audio_track: None,
                    }]
                )
                .is_err()
        );
    }

    #[test]
    fn audio_rejects_timeline_global_colour_and_reviewer_rejects_all_mutations() {
        let document = fixture();
        let audio_policy = policy(&document, Role::Audio);
        for operation in [
            Operation::RippleDeleteClip { clip: ClipId(2) },
            Operation::SplitClip {
                clip: ClipId(2),
                at: TimeCode(20),
            },
            Operation::SetTrackSyncLock {
                track: TrackId(2),
                locked: true,
            },
            Operation::SetColorContext {
                color_context: document.color_context.clone(),
            },
            Operation::SetClipSpeed {
                clip: ClipId(2),
                speed_percent: 100,
            },
        ] {
            assert!(
                audio_policy
                    .validate_batch(&document, &[operation])
                    .is_err()
            );
        }
        assert!(
            policy(&document, Role::Reviewer)
                .validate_batch(&document, &[audio(2, 10)])
                .is_err()
        );
    }

    #[test]
    fn editor_ripple_checks_actual_collateral_tracks_and_markers() {
        let mut document = fixture();
        Operation::AddMarker {
            marker: kinewright_core::Marker {
                id: kinewright_core::MarkerId(1),
                position: TimeCode(120),
                label: "note".into(),
                color_token: 0,
            },
        }
        .apply(&mut document)
        .unwrap();
        let operation = Operation::RippleDeleteClip { clip: ClipId(2) };
        let allowed = policy(&document, Role::Editor);
        allowed
            .validate_batch(&document, std::slice::from_ref(&operation))
            .unwrap();
        let mut applied = document.clone();
        apply_batch(&mut applied, std::slice::from_ref(&operation)).unwrap();
        assert_eq!(
            applied.clip(ClipId(1)).unwrap().timeline_start,
            TimeCode(60)
        );
        assert_eq!(applied.markers[0].position, TimeCode(60));
        for excluded in ["track:1", "clip:3", "marker:1"] {
            let mut narrowed = allowed.clone();
            narrowed.scope.objects.remove(excluded);
            assert!(
                narrowed
                    .validate_batch(&document, std::slice::from_ref(&operation))
                    .is_err(),
                "collateral {excluded}"
            );
        }
        let mut narrowed = allowed;
        narrowed.scope.fields.remove("document.duration");
        assert!(narrowed.validate_batch(&document, &[operation]).is_err());
    }

    #[test]
    fn shared_bus_and_sidechain_closure_is_transitive_and_checks_old_routing() {
        let mut document = fixture();
        Operation::UpsertAudioBus {
            bus: bus(1, &[2], &[3]),
        }
        .apply(&mut document)
        .unwrap();
        let allowed = policy(&document, Role::Audio);
        allowed.validate_batch(&document, &[audio(2, -50)]).unwrap();
        for excluded in ["bus:1", "track:3"] {
            let mut narrowed = allowed.clone();
            narrowed.scope.bus_sidechain_closure.remove(excluded);
            assert!(
                narrowed
                    .validate_batch(&document, &[audio(2, -50)])
                    .is_err()
            );
        }
        let mut narrowed = allowed.clone();
        narrowed.scope.tracks.remove("track:3");
        assert!(
            narrowed
                .validate_batch(
                    &document,
                    &[Operation::UpsertAudioBus {
                        bus: bus(1, &[2], &[])
                    }]
                )
                .is_err()
        );
        Operation::UpsertAudioBus {
            bus: bus(2, &[3], &[1]),
        }
        .apply(&mut document)
        .unwrap();
        assert!(
            policy(&document, Role::Audio)
                .validate_batch(&document, &[audio(2, -50)])
                .is_err()
        );
    }

    #[test]
    fn solo_and_shared_master_cannot_change_video_audio_indirectly() {
        let mut document = fixture();
        let solo = Operation::SetTrackMix {
            track: TrackId(2),
            gain_tenth_db: 0,
            pan_percent: 0,
            mute: false,
            solo: true,
        };
        assert!(
            policy(&document, Role::Audio)
                .validate_batch(&document, &[solo])
                .is_err()
        );
        Operation::SetAudioMaster {
            master: kinewright_core::AudioMaster {
                gain_tenth_db: 0,
                gain_curve: None,
                effects: vec![effect("audio_compressor", 1)],
            },
        }
        .apply(&mut document)
        .unwrap();
        assert!(
            policy(&document, Role::Audio)
                .validate_batch(&document, &[audio(2, -80)])
                .is_err()
        );
    }

    #[test]
    fn effect_additions_and_existing_effect_mutations_use_real_registry_classes() {
        let mut document = fixture();
        let transform = Operation::AddEffect {
            clip: ClipId(1),
            effect: effect("transform", 1),
        };
        let primary = Operation::AddEffect {
            clip: ClipId(1),
            effect: effect("primary_correction", 2),
        };
        policy(&document, Role::Motion)
            .validate_batch(&document, std::slice::from_ref(&transform))
            .unwrap();
        assert!(
            policy(&document, Role::Colour)
                .validate_batch(&document, std::slice::from_ref(&transform))
                .is_err()
        );
        policy(&document, Role::Colour)
            .validate_batch(&document, std::slice::from_ref(&primary))
            .unwrap();
        assert!(
            policy(&document, Role::Motion)
                .validate_batch(&document, std::slice::from_ref(&primary))
                .is_err()
        );
        apply_batch(&mut document, &[transform, primary]).unwrap();
        for operation in [
            Operation::SetEffectParam {
                clip: ClipId(1),
                effect: EffectId(2),
                name: "exposure_milli_stops".into(),
                value: ParamValue::Integer(0),
            },
            Operation::SetEffectEnabled {
                clip: ClipId(1),
                effect: EffectId(2),
                enabled: true,
            },
            Operation::RemoveEffect {
                clip: ClipId(1),
                effect: EffectId(2),
            },
        ] {
            assert!(
                policy(&document, Role::Motion)
                    .validate_batch(&document, &[operation])
                    .is_err()
            );
        }
        assert!(
            policy(&document, Role::Colour)
                .validate_batch(
                    &document,
                    &[Operation::SetEffectEnabled {
                        clip: ClipId(1),
                        effect: EffectId(1),
                        enabled: true
                    }]
                )
                .is_err()
        );
        assert!(
            policy(&document, Role::Director)
                .validate_batch(
                    &document,
                    &[Operation::AddEffect {
                        clip: ClipId(1),
                        effect: effect("future_unclassified_effect", 3)
                    }]
                )
                .is_err()
        );
    }

    #[test]
    fn effect_copy_checks_requested_names_even_when_absent_or_no_op() {
        let mut document = fixture();
        Operation::AddEffect {
            clip: ClipId(1),
            effect: effect("transform", 1),
        }
        .apply(&mut document)
        .unwrap();
        let motion = policy(&document, Role::Motion);
        let copy = |names| Operation::CopyClipAttributes {
            from_clip: ClipId(1),
            to_clip: ClipId(1),
            names,
            include_keyframes: true,
        };
        motion
            .validate_batch(&document, &[copy(Some(vec!["transform".into()]))])
            .unwrap();
        assert!(
            motion
                .validate_batch(&document, &[copy(Some(vec!["primary_correction".into()]))])
                .is_err()
        );
        assert!(
            motion
                .validate_batch(&document, &[copy(Some(vec!["opacity".into()]))])
                .is_err()
        );
        assert!(
            policy(&document, Role::Colour)
                .validate_batch(&document, &[copy(None)])
                .is_err()
        );
    }

    #[test]
    fn imports_relinks_and_file_backed_effects_need_authority_outside_this_api() {
        let document = fixture();
        for role in [
            Role::Director,
            Role::Editor,
            Role::Motion,
            Role::Audio,
            Role::Colour,
            Role::Reviewer,
        ] {
            let mut policy = policy(&document, role);
            policy
                .scope
                .consent_refs
                .push(policy.scope.base.checkpoint.clone());
            policy
                .scope
                .asset_roots
                .push(policy.scope.base.checkpoint.clone());
            assert!(
                policy
                    .validate_batch(
                        &document,
                        &[Operation::AddAsset {
                            asset: document.media_pool[0].clone()
                        }]
                    )
                    .is_err()
            );
            assert!(
                policy
                    .validate_batch(
                        &document,
                        &[Operation::RelinkAsset {
                            asset: AssetId(1),
                            candidate: kinewright_core::RelinkCandidate {
                                path: "/unadmitted/input.mp4".into(),
                                fingerprint: MediaSourceFingerprint::default(),
                                kind: MediaKind::AudioVideo,
                                fps: document.fps,
                                duration: TimeCode(600),
                                resolution: Some(document.resolution),
                            },
                            allow_unverified_source: true
                        }]
                    )
                    .is_err()
            );
        }
        let mut external = effect("cube_lut", 1);
        external.parameters.insert(
            "path".into(),
            ParamValue::Text("/unadmitted/look.cube".into()),
        );
        assert!(
            policy(&document, Role::Colour)
                .validate_batch(
                    &document,
                    &[Operation::AddEffect {
                        clip: ClipId(1),
                        effect: external
                    }]
                )
                .is_err()
        );
    }

    #[test]
    fn clip_shaping_can_exclude_untouched_clips_but_track_shaping_cannot() {
        let mut document = fixture();
        Operation::AddClip {
            track: TrackId(2),
            asset: AssetId(1),
            at: TimeCode(120),
            source: TimeCode(0)..TimeCode(60),
        }
        .apply(&mut document)
        .unwrap();
        let mut scoped = policy(&document, Role::Audio);
        scoped.scope.objects.remove("clip:4");
        scoped.validate_batch(&document, &[audio(2, -80)]).unwrap();
        assert!(
            scoped
                .validate_batch(
                    &document,
                    &[Operation::SetTrackMix {
                        track: TrackId(2),
                        gain_tenth_db: -80,
                        pan_percent: 0,
                        mute: false,
                        solo: false,
                    }]
                )
                .is_err()
        );
    }

    #[test]
    fn visual_only_effects_do_not_claim_unrelated_audio_routing() {
        let mut document = fixture();
        Operation::UpsertAudioBus {
            bus: bus(1, &[1, 2], &[]),
        }
        .apply(&mut document)
        .unwrap();
        let mut motion = policy(&document, Role::Motion);
        motion.scope.tracks.remove("track:2");
        motion.scope.objects.remove("clip:2");
        motion.scope.objects.remove("bus:1");
        motion.scope.bus_sidechain_closure.clear();
        motion
            .validate_batch(
                &document,
                &[Operation::AddEffect {
                    clip: ClipId(1),
                    effect: effect("transform", 1),
                }],
            )
            .unwrap();
    }

    #[test]
    fn identical_effect_copy_still_checks_target_identity_and_parameter_fields() {
        let mut document = fixture();
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at: TimeCode(240),
            source: TimeCode(0)..TimeCode(60),
        }
        .apply(&mut document)
        .unwrap();
        let mut transform = effect("transform", 1);
        transform
            .parameters
            .insert("x_percent".into(), ParamValue::Integer(0));
        Operation::AddEffect {
            clip: ClipId(1),
            effect: transform.clone(),
        }
        .apply(&mut document)
        .unwrap();
        transform.id = EffectId(2);
        Operation::AddEffect {
            clip: ClipId(4),
            effect: transform,
        }
        .apply(&mut document)
        .unwrap();
        let operation = Operation::CopyClipAttributes {
            from_clip: ClipId(1),
            to_clip: ClipId(4),
            names: None,
            include_keyframes: false,
        };
        let allowed = policy(&document, Role::Motion);
        allowed
            .validate_batch(&document, std::slice::from_ref(&operation))
            .unwrap();
        let mut actual = document.clone();
        apply_batch(&mut actual, std::slice::from_ref(&operation)).unwrap();
        assert_eq!(actual, document);
        let mut narrowed = allowed.clone();
        narrowed.scope.objects.remove("effect:4:2");
        assert!(
            narrowed
                .validate_batch(&document, std::slice::from_ref(&operation))
                .is_err()
        );
        let mut narrowed = allowed;
        narrowed.scope.fields.remove("effect.parameters.x_percent");
        assert!(narrowed.validate_batch(&document, &[operation]).is_err());
    }

    #[test]
    fn generated_identities_and_each_intermediate_batch_effect_need_scope() {
        let document = fixture();
        let operations = [
            Operation::SplitClip {
                clip: ClipId(2),
                at: TimeCode(20),
            },
            audio(4, -80),
        ];
        assert!(
            policy(&document, Role::Editor)
                .validate_batch(&document, &operations)
                .is_err()
        );
        let allowed = policy(&document, Role::Director);
        allowed.validate_batch(&document, &operations).unwrap();
        let mut applied = document.clone();
        apply_batch(&mut applied, &operations).unwrap();
        assert_eq!(applied.clip(ClipId(4)).unwrap().audio_gain_tenth_db, -80);
        let mut narrowed = allowed;
        narrowed.scope.objects.remove("clip:4");
        assert!(narrowed.validate_batch(&document, &operations).is_err());
        assert!(document.clip(ClipId(4)).is_none());
    }

    #[test]
    fn director_has_no_implicit_scope_and_unknown_domains_are_refused() {
        let document = fixture();
        let mut director = policy(&document, Role::Director);
        director.scope.objects.remove("clip:2");
        assert!(
            director
                .validate_batch(&document, &[audio(2, -50)])
                .is_err()
        );
        for invalid in ["operation", "object", "field", "track", "closure"] {
            let mut scoped = policy(&document, Role::Editor);
            match invalid {
                "operation" => {
                    scoped.scope.operations.insert("future_operation".into());
                }
                "object" => {
                    scoped.scope.objects.insert("*".into());
                }
                "field" => {
                    scoped.scope.fields.insert("clip.future_field".into());
                }
                "track" => {
                    scoped.scope.tracks.insert("track:02".into());
                }
                "closure" => {
                    scoped
                        .scope
                        .bus_sidechain_closure
                        .insert("external:1".into());
                }
                _ => unreachable!(),
            }
            assert!(
                scoped.validate_batch(&document, &[audio(2, -50)]).is_err(),
                "unknown {invalid}"
            );
        }
    }
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one animated split/delete and direct-effect authority oracle"
    )]
    fn editor_structural_edits_preserve_effects_without_direct_effect_authority() {
        let mut document = fixture();
        let mut transform = effect("transform", 1);
        transform
            .parameters
            .insert("x_percent".into(), ParamValue::Integer(0));
        transform.keyframes.insert(
            "x_percent".into(),
            kinewright_core::AutomationCurve {
                keyframes: vec![
                    kinewright_core::Keyframe {
                        at: TimeCode(0),
                        value: 0,
                        interpolation: kinewright_core::KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                    kinewright_core::Keyframe {
                        at: TimeCode(59),
                        value: 20,
                        interpolation: kinewright_core::KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                ],
            },
        );
        apply_batch(
            &mut document,
            &[
                Operation::AddEffect {
                    clip: ClipId(1),
                    effect: transform,
                },
                Operation::AddEffect {
                    clip: ClipId(1),
                    effect: effect("primary_correction", 2),
                },
            ],
        )
        .unwrap();
        let scoped = policy(&document, Role::Editor);
        let split = Operation::SplitClip {
            clip: ClipId(1),
            at: TimeCode(140),
        };
        scoped
            .validate_batch(&document, std::slice::from_ref(&split))
            .unwrap();
        let mut after = document.clone();
        apply_batch(&mut after, std::slice::from_ref(&split)).unwrap();
        assert_eq!(after.clip(ClipId(4)).unwrap().effects.len(), 2);
        scoped
            .validate_batch(&document, &[Operation::DeleteClip { clip: ClipId(1) }])
            .unwrap();
        assert!(
            scoped
                .validate_batch(
                    &document,
                    &[Operation::SetEffectParam {
                        clip: ClipId(1),
                        effect: EffectId(1),
                        name: "x_percent".into(),
                        value: ParamValue::Integer(5)
                    }]
                )
                .is_err()
        );
        assert!(
            scoped
                .validate_batch(
                    &document,
                    &[Operation::RemoveEffect {
                        clip: ClipId(1),
                        effect: EffectId(1)
                    }]
                )
                .is_err()
        );
        let mut narrow = scoped.clone();
        narrow.scope.objects.remove("effect:4:1");
        assert!(
            narrow
                .validate_batch(&document, std::slice::from_ref(&split))
                .is_err()
        );
        let mut narrow = scoped;
        narrow.scope.fields.remove("effect.parameters.x_percent");
        assert!(narrow.validate_batch(&document, &[split]).is_err());
    }
}
