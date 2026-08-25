//! Human-facing media availability, relink, and cache controls.
//!
//! The Core document remains the source of truth for edits.  This module owns
//! the machine-local observations and the small amount of asynchronous UI
//! state needed to turn a probed replacement into exactly one
//! `RelinkAsset` operation.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use eframe::egui;
use kinewright_core::{
    AssetId, ClipContent, ClipId, ColorStage, Command, Document, EffectId, LutAsset, LutAssetId,
    MediaAsset, MediaAvailabilityKind, MediaAvailabilityStatus, MediaCacheClearResult,
    MediaCacheFamily, MediaCacheFamilyStatus, MediaError, MediaSourceFingerprint, Operation,
    RelinkCandidate, ThreePointMode, TimeCode, TimelineRevision, TrackId,
};
use kinewright_media::{LutAssetImport, LutStore};

use crate::{
    app::KinewrightApp,
    project::session_index_by_id,
    theme::{self, color, space, type_size},
};

#[derive(Debug)]
pub(crate) struct RelinkProbeResponse {
    session_id: u64,
    asset_id: AssetId,
    expected_revision: TimelineRevision,
    path: PathBuf,
    result: Result<MediaAsset, MediaError>,
}

/// A selected Source viewer revalidates a cached verified observation at most
/// once every five minutes. Availability hashes the entire source, so this
/// deliberately conservative bound prevents stale display pixels without
/// turning an open viewer into a continuous large-file hashing loop. An edit
/// click always requires a current asynchronous verification (or attaches to
/// the one already in flight for this exact source).
pub(crate) const SOURCE_VERIFIED_FRESHNESS: Duration = Duration::from_mins(5);

#[derive(Debug, Clone)]
pub(crate) struct MediaStatusResponse {
    pub(crate) session_id: u64,
    pub(crate) asset_id: AssetId,
    pub(crate) request_id: u64,
    pub(crate) path: PathBuf,
    pub(crate) fingerprint: MediaSourceFingerprint,
    pub(crate) status: MediaAvailabilityStatus,
}
pub(crate) type CacheClearResponse = (MediaCacheFamily, Result<MediaCacheClearResult, MediaError>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingSourceEdit {
    pub(crate) session_id: u64,
    pub(crate) request_id: u64,
    pub(crate) asset_id: AssetId,
    pub(crate) path: PathBuf,
    pub(crate) fingerprint: MediaSourceFingerprint,
    pub(crate) expected_revision: TimelineRevision,
    pub(crate) selected_asset: Option<AssetId>,
    pub(crate) source_position: TimeCode,
    pub(crate) timeline_in: TimeCode,
    pub(crate) source_in: TimeCode,
    pub(crate) source_out: TimeCode,
    pub(crate) video_target: Option<TrackId>,
    pub(crate) audio_target: Option<TrackId>,
    pub(crate) mode: ThreePointMode,
}

impl PendingSourceEdit {
    #[must_use]
    pub(crate) fn matches_response(&self, response: &MediaStatusResponse) -> bool {
        self.session_id == response.session_id
            && self.request_id == response.request_id
            && self.asset_id == response.asset_id
            && self.path == response.path
            && self.fingerprint == response.fingerprint
    }
}

/// The live, ephemeral state that must remain unchanged while a human edit is
/// waiting for its mandatory source verification. This deliberately lives
/// outside the serialized document so it can be compared without constructing
/// egui widgets in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceEditContext {
    pub(crate) session_id: u64,
    pub(crate) asset_id: AssetId,
    pub(crate) path: PathBuf,
    pub(crate) fingerprint: MediaSourceFingerprint,
    pub(crate) revision: TimelineRevision,
    pub(crate) selected_asset: Option<AssetId>,
    pub(crate) source_position: TimeCode,
    pub(crate) timeline_in: TimeCode,
    pub(crate) source_in: TimeCode,
    pub(crate) source_out: TimeCode,
    pub(crate) video_target: Option<TrackId>,
    pub(crate) audio_target: Option<TrackId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceEditDispatchIntent {
    pub(crate) session_id: u64,
    pub(crate) expected_revision: TimelineRevision,
    pub(crate) operation: Operation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceEditRejection {
    SupersededResponse,
    SourceNoLongerVerified,
    SourceMissing,
    SessionChanged,
    SourceSelectionChanged,
    AssetIdentityChanged,
    RevisionChanged,
    TimelinePositionChanged,
    SourcePositionChanged,
    SourceMarksChanged,
    SourceRoutesChanged,
}

impl SourceEditRejection {
    fn message(self) -> &'static str {
        match self {
            Self::SupersededResponse => {
                "Source verification was superseded before the edit could be applied"
            }
            Self::SourceNoLongerVerified => {
                "Source verification did not confirm the original online source; no edit was applied"
            }
            Self::SourceMissing => {
                "Selected source asset is no longer available; no edit was applied"
            }
            Self::SessionChanged => "Project session changed while Source was being verified",
            Self::SourceSelectionChanged => {
                "Source selection changed while Source was being verified; no edit was applied"
            }
            Self::AssetIdentityChanged => {
                "Source file identity changed while Source was being verified; no edit was applied"
            }
            Self::RevisionChanged => {
                "Timeline changed while Source was being verified; no edit was applied"
            }
            Self::TimelinePositionChanged => {
                "Program position changed while Source was being verified; no edit was applied"
            }
            Self::SourcePositionChanged => {
                "Source position changed while Source was being verified; no edit was applied"
            }
            Self::SourceMarksChanged => {
                "Source In/Out changed while Source was being verified; no edit was applied"
            }
            Self::SourceRoutesChanged => {
                "Patch destinations changed while Source was being verified; no edit was applied"
            }
        }
    }
}

/// Create the single Core command that may follow a completed Source
/// revalidation. Every field is checked before this intent exists, leaving the
/// caller no fail-open route around the async boundary.
pub(crate) fn source_edit_dispatch_intent(
    pending: &PendingSourceEdit,
    response: &MediaStatusResponse,
    current: Option<&SourceEditContext>,
) -> Result<SourceEditDispatchIntent, SourceEditRejection> {
    if !pending.matches_response(response) {
        return Err(SourceEditRejection::SupersededResponse);
    }
    if response.status.kind != MediaAvailabilityKind::OnlineVerified {
        return Err(SourceEditRejection::SourceNoLongerVerified);
    }
    let Some(current) = current else {
        return Err(SourceEditRejection::SourceMissing);
    };
    if current.session_id != pending.session_id {
        return Err(SourceEditRejection::SessionChanged);
    }
    if current.selected_asset != pending.selected_asset
        || current.selected_asset != Some(pending.asset_id)
    {
        return Err(SourceEditRejection::SourceSelectionChanged);
    }
    if current.asset_id != pending.asset_id
        || current.path != pending.path
        || current.fingerprint != pending.fingerprint
    {
        return Err(SourceEditRejection::AssetIdentityChanged);
    }
    if current.revision != pending.expected_revision {
        return Err(SourceEditRejection::RevisionChanged);
    }
    if current.timeline_in != pending.timeline_in {
        return Err(SourceEditRejection::TimelinePositionChanged);
    }
    if current.source_position != pending.source_position {
        return Err(SourceEditRejection::SourcePositionChanged);
    }
    if current.source_in != pending.source_in || current.source_out != pending.source_out {
        return Err(SourceEditRejection::SourceMarksChanged);
    }
    if current.video_target != pending.video_target || current.audio_target != pending.audio_target
    {
        return Err(SourceEditRejection::SourceRoutesChanged);
    }
    Ok(SourceEditDispatchIntent {
        session_id: pending.session_id,
        expected_revision: pending.expected_revision,
        operation: Operation::PatchedThreePointEdit {
            asset: pending.asset_id,
            source_in: Some(pending.source_in),
            source_out: Some(pending.source_out),
            timeline_in: Some(pending.timeline_in),
            timeline_out: None,
            mode: pending.mode,
            video_track: pending.video_target,
            audio_track: pending.audio_target,
        },
    })
}

/// Consume a matching pending edit exactly once. A late response from an
/// older generation cannot consume a newer edit, and a duplicate response
/// cannot produce a second dispatch intent.
pub(crate) fn take_source_edit_dispatch_intent(
    pending_edit: &mut Option<PendingSourceEdit>,
    response: &MediaStatusResponse,
    current: Option<&SourceEditContext>,
) -> Result<Option<SourceEditDispatchIntent>, SourceEditRejection> {
    let Some(pending) = pending_edit.as_ref() else {
        return Ok(None);
    };
    if !pending.matches_response(response) {
        return Ok(None);
    }
    let pending = pending_edit
        .take()
        .expect("a matching pending Source edit must still be present");
    source_edit_dispatch_intent(&pending, response, current).map(Some)
}

pub(crate) const SOURCE_EDIT_REFRESH_CANCELED_MESSAGE: &str =
    "Source edit canceled by media refresh; no edit was applied";

/// Cancel the human edit tied to a status generation before that generation
/// is invalidated. A refresh may drop the old response entirely, so waiting
/// for `handle_media_status_response` to clear this slot would leave Source
/// controls locked forever.
#[must_use]
pub(crate) fn cancel_pending_source_edit_for_session(
    pending_edit: &mut Option<PendingSourceEdit>,
    session_id: u64,
) -> bool {
    if pending_edit
        .as_ref()
        .is_some_and(|pending| pending.session_id == session_id)
    {
        pending_edit.take();
        true
    } else {
        false
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SourceMediaStatus {
    pub(crate) status: Option<MediaAvailabilityStatus>,
    pub(crate) refresh_after: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingLegacyRelink {
    pub(crate) session_id: u64,
    pub(crate) asset_id: AssetId,
    pub(crate) expected_revision: TimelineRevision,
    pub(crate) candidate: RelinkCandidate,
    pub(crate) asset_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RelinkRevisionConflict {
    pub(crate) expected: TimelineRevision,
    pub(crate) actual: TimelineRevision,
}

impl RelinkRevisionConflict {
    fn message(self, project_name: &str) -> String {
        format!(
            "Relink cancelled in {project_name}: the project changed while the replacement was being checked (expected timeline revision {}, current revision is {}). Choose Relink again.",
            self.expected, self.actual
        )
    }
}

/// Check the revision captured before the file picker result was probed.
/// Core repeats this gate atomically when it receives the operation.
pub(crate) fn validate_relink_revision(
    expected: TimelineRevision,
    actual: TimelineRevision,
) -> Result<(), RelinkRevisionConflict> {
    if expected == actual {
        Ok(())
    } else {
        Err(RelinkRevisionConflict { expected, actual })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelinkPreflight {
    Ready,
    NeedsLegacyConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RelinkRejection {
    CandidateUnverified,
    FingerprintMismatch {
        expected: MediaSourceFingerprint,
        candidate: MediaSourceFingerprint,
    },
}

impl RelinkRejection {
    pub(crate) fn message(&self, asset: AssetId) -> String {
        match self {
            Self::CandidateUnverified => format!(
                "Cannot relink asset {asset}: the replacement did not produce a verified source fingerprint"
            ),
            Self::FingerprintMismatch {
                expected,
                candidate,
            } => format!(
                "Cannot relink asset {asset}: source fingerprint mismatch (expected {}, candidate {})",
                fingerprint_summary(expected),
                fingerprint_summary(candidate)
            ),
        }
    }
}

/// Build the Core candidate from probe output while deliberately ignoring the
/// probe's generated asset id.  Relink targets the existing asset id carried by
/// the operation, so no duplicate asset can be created by this workflow.
#[must_use]
pub(crate) fn build_relink_candidate(path: PathBuf, probed: &MediaAsset) -> RelinkCandidate {
    RelinkCandidate {
        path,
        fingerprint: probed.source_fingerprint.clone(),
        kind: probed.kind,
        fps: probed.fps,
        duration: probed.duration,
        resolution: probed.resolution,
    }
}

/// Perform the UI-side identity gate before dispatching Core.  Core repeats
/// the same validation, but this gate lets the human see a useful error and
/// ensures legacy projects never silently opt into an unverified relink.
pub(crate) fn preflight_relink(
    target: &MediaAsset,
    candidate: &RelinkCandidate,
) -> Result<RelinkPreflight, RelinkRejection> {
    if !candidate.fingerprint.is_verified() {
        return Err(RelinkRejection::CandidateUnverified);
    }
    if target.source_fingerprint.is_verified() {
        if target.source_fingerprint == candidate.fingerprint {
            Ok(RelinkPreflight::Ready)
        } else {
            Err(RelinkRejection::FingerprintMismatch {
                expected: target.source_fingerprint.clone(),
                candidate: candidate.fingerprint.clone(),
            })
        }
    } else {
        Ok(RelinkPreflight::NeedsLegacyConfirmation)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceDisplayState {
    Checking,
    OnlineVerified,
    OnlineUnverified,
    Offline,
    Changed,
    Unreadable,
}

impl SourceDisplayState {
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Checking => "CHECKING SOURCE…",
            Self::OnlineVerified => "ONLINE · VERIFIED",
            Self::OnlineUnverified => "ONLINE · UNVERIFIED",
            Self::Offline => "OFFLINE",
            Self::Changed => "CHANGED",
            Self::Unreadable => "UNREADABLE",
        }
    }

    #[must_use]
    pub(crate) const fn is_warning(self) -> bool {
        !matches!(self, Self::OnlineVerified)
    }

    #[must_use]
    pub(crate) const fn blocks_preview(self) -> bool {
        matches!(self, Self::Offline | Self::Changed | Self::Unreadable)
    }

    /// Source imagery and source edits require a live, identity-verified
    /// observation.  This is intentionally stricter than `blocks_preview`:
    /// Program may continue showing its current live frame while a source
    /// status check is pending or while a legacy source is unverified, but the
    /// Source viewer must never request or reuse pixels in those states.
    #[must_use]
    pub(crate) const fn allows_verified_source_access(self) -> bool {
        matches!(self, Self::OnlineVerified)
    }

    #[must_use]
    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::Checking => "Checking source availability…",
            Self::OnlineVerified => "Source is readable and matches its imported fingerprint.",
            Self::OnlineUnverified => {
                "Source is readable, but this legacy asset has no verified fingerprint."
            }
            Self::Offline => "Source file is missing. Playback will not substitute black frames.",
            Self::Changed => {
                "The file at this path changed. Relink to the original source before playback."
            }
            Self::Unreadable => {
                "The source cannot be read. Check permissions or relink to a readable file."
            }
        }
    }
}

#[must_use]
pub(crate) fn source_display_state(status: Option<&MediaAvailabilityStatus>) -> SourceDisplayState {
    status.map_or(SourceDisplayState::Checking, |status| match status.kind {
        MediaAvailabilityKind::OnlineVerified => SourceDisplayState::OnlineVerified,
        MediaAvailabilityKind::OnlineUnverified => SourceDisplayState::OnlineUnverified,
        MediaAvailabilityKind::OfflineMissing => SourceDisplayState::Offline,
        MediaAvailabilityKind::Changed => SourceDisplayState::Changed,
        MediaAvailabilityKind::Unreadable => SourceDisplayState::Unreadable,
    })
}

/// Decide whether a cached source frame is safe to display for the current
/// availability observation.  Keeping the `has_texture` part in this pure
/// helper makes the stale-texture rule explicit at the Source viewer boundary:
/// no cached texture is valid unless the current observation is verified.
#[must_use]
pub(crate) const fn should_display_source_texture(
    state: SourceDisplayState,
    has_texture: bool,
) -> bool {
    has_texture && state.allows_verified_source_access()
}

/// A forced edit revalidation is stricter than a cached verified observation:
/// even previously decoded Source pixels stay hidden until its exact response
/// arrives.
#[must_use]
pub(crate) const fn source_access_is_allowed(
    state: SourceDisplayState,
    revalidation_pending: bool,
) -> bool {
    !revalidation_pending && state.allows_verified_source_access()
}

/// Source edit eligibility is kept independent of egui so the UI and its
/// tests share the same fail-closed availability rule.  Dispatch repeats the
/// availability check against the current live status before sending Core.
#[must_use]
pub(crate) const fn source_edit_is_eligible(
    state: SourceDisplayState,
    duration: i64,
    source_in: i64,
    source_out: i64,
    route_valid: bool,
) -> bool {
    state.allows_verified_source_access()
        && duration > 0
        && source_in >= 0
        && source_out > source_in
        && source_out <= duration
        && route_valid
}

/// The forced availability generation freezes Source controls as well as the
/// image surface. This avoids a second click or a changed ephemeral context
/// while the exact edit request is being checked.
#[must_use]
pub(crate) const fn source_edit_controls_are_enabled(
    state: SourceDisplayState,
    duration: i64,
    source_in: i64,
    source_out: i64,
    route_valid: bool,
    revalidation_pending: bool,
) -> bool {
    !revalidation_pending
        && source_edit_is_eligible(state, duration, source_in, source_out, route_valid)
}

#[must_use]
const fn availability_invalidates_visuals(status: &MediaAvailabilityStatus) -> bool {
    matches!(status.kind, MediaAvailabilityKind::Changed)
}

pub(crate) fn paint_source_status(ui: &mut egui::Ui, state: SourceDisplayState) {
    let status_color = if state.is_warning() {
        if state.blocks_preview() {
            color::STATUS_DANGER
        } else {
            color::STATUS_WARNING
        }
    } else {
        color::STATUS_SUCCESS
    };
    ui.colored_label(
        status_color,
        egui::RichText::new(state.label()).font(theme::semibold(type_size::CAPTION)),
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachePresentation {
    pub(crate) title: &'static str,
    pub(crate) persistence: &'static str,
    pub(crate) description: &'static str,
    pub(crate) clearable: bool,
}

#[must_use]
pub(crate) fn cache_presentation(status: &MediaCacheFamilyStatus) -> CachePresentation {
    match status.family {
        MediaCacheFamily::PreviewMemory => CachePresentation {
            title: "Preview memory",
            persistence: "Ephemeral · memory only",
            description: "Scaled decode frames used for interactive playback; no generated proxy file exists.",
            clearable: status.supported,
        },
        MediaCacheFamily::VisualAssets => CachePresentation {
            title: "Visual assets",
            persistence: "Persistent · derived cache",
            description: "Content-addressed thumbnails and waveforms; regenerable from source media.",
            clearable: status.supported,
        },
        MediaCacheFamily::DerivedAnalysis => CachePresentation {
            title: "Derived analysis",
            persistence: "Persistent · derived cache",
            description: "Content-addressed silence, scene, and beat results; regenerable from source media.",
            clearable: status.supported,
        },
        MediaCacheFamily::Transcripts => CachePresentation {
            title: "Transcripts",
            persistence: "Persistent · derived cache",
            description: "Content-addressed speech recognition results; regenerable from source media.",
            clearable: status.supported,
        },
        MediaCacheFamily::GeneratedProxy => CachePresentation {
            title: "Generated proxy",
            persistence: "Unsupported",
            description: "Kinewright does not generate proxy media files in this milestone.",
            clearable: false,
        },
    }
}

#[derive(Debug, Default)]
pub(crate) struct MediaStatusStore {
    entries: HashMap<(u64, AssetId), MediaStatusEntry>,
    pending: HashMap<(u64, AssetId, PathBuf), u64>,
    next_request_id: u64,
}

#[derive(Debug, Clone)]
struct MediaStatusEntry {
    path: PathBuf,
    fingerprint: MediaSourceFingerprint,
    status: MediaAvailabilityStatus,
    observed_at: Instant,
}

impl MediaStatusStore {
    pub(crate) fn status(
        &self,
        session_id: u64,
        asset: &MediaAsset,
    ) -> Option<MediaAvailabilityStatus> {
        self.entries
            .get(&(session_id, asset.id))
            .filter(|entry| {
                entry.path == asset.path && entry.fingerprint == asset.source_fingerprint
            })
            .map(|entry| entry.status.clone())
    }

    /// Read the Source viewer's status with a bounded verified-observation
    /// lifetime.  Only `OnlineVerified` expires automatically: failure states
    /// remain stable until refresh/relink, while the expensive success path is
    /// periodically proven again.  Expiry removes the entry before returning,
    /// so the caller paints Checking and cannot retrieve a cached texture.
    fn source_status_at(
        &mut self,
        session_id: u64,
        asset: &MediaAsset,
        now: Instant,
    ) -> SourceMediaStatus {
        let key = (session_id, asset.id);
        let Some(entry) = self.entries.get(&key) else {
            return SourceMediaStatus {
                status: None,
                refresh_after: None,
            };
        };
        if entry.path != asset.path || entry.fingerprint != asset.source_fingerprint {
            self.entries.remove(&key);
            return SourceMediaStatus {
                status: None,
                refresh_after: None,
            };
        }
        if entry.status.kind != MediaAvailabilityKind::OnlineVerified {
            return SourceMediaStatus {
                status: Some(entry.status.clone()),
                refresh_after: None,
            };
        }
        let age = now.saturating_duration_since(entry.observed_at);
        if age >= SOURCE_VERIFIED_FRESHNESS {
            self.entries.remove(&key);
            SourceMediaStatus {
                status: None,
                refresh_after: None,
            }
        } else {
            SourceMediaStatus {
                status: Some(entry.status.clone()),
                refresh_after: Some(
                    SOURCE_VERIFIED_FRESHNESS
                        .checked_sub(age)
                        .expect("verified observation age was checked against its freshness bound"),
                ),
            }
        }
    }

    fn next_request_id(&mut self) -> u64 {
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.next_request_id
    }

    pub(crate) fn begin(&mut self, session_id: u64, asset: &MediaAsset) -> Option<u64> {
        if self.status(session_id, asset).is_some() {
            return None;
        }
        let key = (session_id, asset.id, asset.path.clone());
        if self.pending.contains_key(&key) {
            return None;
        }
        let request_id = self.next_request_id();
        self.pending.insert(key, request_id);
        Some(request_id)
    }

    /// The sole in-flight availability generation for this exact source, if
    /// any. Edit clicks attach to this full-file check instead of spawning a
    /// second concurrent hash of the same media.
    pub(crate) fn pending_request_id(&self, session_id: u64, asset: &MediaAsset) -> Option<u64> {
        self.pending
            .get(&(session_id, asset.id, asset.path.clone()))
            .copied()
    }

    /// Start a new generation when a cached observation exists, or join an
    /// existing one for this exact source. This makes repeated clicks and
    /// display-expiry checks incapable of launching concurrent full-file
    /// hashes. When a new generation is required, the old observation is
    /// removed immediately and older responses are rejected by request id.
    pub(crate) fn begin_forced(&mut self, session_id: u64, asset: &MediaAsset) -> u64 {
        if let Some(request_id) = self.pending_request_id(session_id, asset) {
            return request_id;
        }
        self.entries.remove(&(session_id, asset.id));
        self.pending.retain(|(session, pending_asset, _), _| {
            *session != session_id || *pending_asset != asset.id
        });
        let request_id = self.next_request_id();
        self.pending
            .insert((session_id, asset.id, asset.path.clone()), request_id);
        request_id
    }

    pub(crate) fn accepts_response(
        &self,
        session_id: u64,
        asset_id: AssetId,
        path: &std::path::Path,
        request_id: u64,
    ) -> bool {
        self.pending
            .get(&(session_id, asset_id, path.to_path_buf()))
            .is_some_and(|pending_id| *pending_id == request_id)
    }

    pub(crate) fn finish(
        &mut self,
        session_id: u64,
        asset_id: AssetId,
        request_id: u64,
        path: PathBuf,
        fingerprint: MediaSourceFingerprint,
        status: MediaAvailabilityStatus,
    ) -> bool {
        self.finish_at(
            session_id,
            asset_id,
            request_id,
            path,
            fingerprint,
            status,
            Instant::now(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_at(
        &mut self,
        session_id: u64,
        asset_id: AssetId,
        request_id: u64,
        path: PathBuf,
        fingerprint: MediaSourceFingerprint,
        status: MediaAvailabilityStatus,
        observed_at: Instant,
    ) -> bool {
        if !self.accepts_response(session_id, asset_id, &path, request_id) {
            return false;
        }
        self.pending.remove(&(session_id, asset_id, path.clone()));
        self.entries.insert(
            (session_id, asset_id),
            MediaStatusEntry {
                path,
                fingerprint,
                status,
                observed_at,
            },
        );
        true
    }

    pub(crate) fn cancel_request(
        &mut self,
        session_id: u64,
        asset_id: AssetId,
        path: &std::path::Path,
        request_id: u64,
    ) {
        let key = (session_id, asset_id, path.to_path_buf());
        if self
            .pending
            .get(&key)
            .is_some_and(|pending_id| *pending_id == request_id)
        {
            self.pending.remove(&key);
        }
    }

    pub(crate) fn invalidate(&mut self, session_id: u64, asset_id: AssetId) {
        self.entries.remove(&(session_id, asset_id));
        self.pending
            .retain(|(session, asset, _), _| *session != session_id || *asset != asset_id);
    }

    pub(crate) fn remove_session(&mut self, session_id: u64) {
        self.entries
            .retain(|(session, _), _| *session != session_id);
        self.pending
            .retain(|(session, _, _), _| *session != session_id);
    }

    pub(crate) fn path_has_changed_observation(&self, path: &std::path::Path) -> bool {
        self.entries.values().any(|entry| {
            entry.path == path && matches!(entry.status.kind, MediaAvailabilityKind::Changed)
        })
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

fn fingerprint_summary(fingerprint: &MediaSourceFingerprint) -> String {
    match (&fingerprint.content_sha256, fingerprint.byte_len) {
        (Some(hash), Some(byte_len)) => format!(
            "{}… ({} bytes)",
            hash.chars().take(12).collect::<String>(),
            byte_len
        ),
        _ => "unknown".to_owned(),
    }
}

/// A successful relink establishes a fresh source observation even when the
/// restored original bytes leave the persisted path and fingerprint unchanged.
#[must_use]
pub(crate) fn media_asset_requires_refresh(
    previous: Option<&MediaAsset>,
    current: &MediaAsset,
    last_operation: Option<&Operation>,
) -> bool {
    previous.is_none_or(|previous| {
        previous.path != current.path || previous.source_fingerprint != current.source_fingerprint
    }) || matches!(
        last_operation,
        Some(Operation::RelinkAsset { asset, .. }) if *asset == current.id
    )
}

#[allow(clippy::cast_precision_loss)]
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

impl KinewrightApp {
    pub(crate) fn choose_relink_for_asset(&mut self, asset_id: AssetId) {
        let Some(asset) = self.focused().document.asset(asset_id).cloned() else {
            self.record_error("Media", format!("Asset {asset_id} no longer exists"));
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Media",
                &[
                    "mp4", "mov", "mkv", "webm", "avi", "wav", "mp3", "flac", "ogg", "m4a", "aac",
                ],
            )
            .pick_file()
        else {
            return;
        };
        self.start_relink_probe(asset.id, path);
    }

    fn start_relink_probe(&mut self, asset_id: AssetId, path: PathBuf) {
        let session_id = self.focused().id;
        let expected_revision = self.focused().revision;
        let media = Arc::clone(&self.analysis);
        let result_tx = self.relink_probe_tx.clone();
        self.relink_probe_pending = self.relink_probe_pending.saturating_add(1);
        self.status = format!("Checking replacement {}…", path.display());
        thread::Builder::new()
            .name("kinewright-relink-probe".to_owned())
            .spawn(move || {
                let result = media.probe(&path);
                let _ = result_tx.send(RelinkProbeResponse {
                    session_id,
                    asset_id,
                    expected_revision,
                    path,
                    result,
                });
            })
            .expect("failed to spawn media relink probe worker");
    }

    fn spawn_media_status_request(
        &mut self,
        session_id: u64,
        asset: &MediaAsset,
        request_id: u64,
    ) -> bool {
        let media = Arc::clone(&self.analysis);
        let status_tx = self.media_status_tx.clone();
        let path = asset.path.clone();
        let fingerprint = asset.source_fingerprint.clone();
        let asset_id = asset.id;
        let asset_snapshot = asset.clone();
        if thread::Builder::new()
            .name("kinewright-media-status".to_owned())
            .spawn(move || {
                let status = media.media_availability(&asset_snapshot);
                let _ = status_tx.send(MediaStatusResponse {
                    session_id,
                    asset_id,
                    request_id,
                    path,
                    fingerprint,
                    status,
                });
            })
            .is_ok()
        {
            true
        } else {
            self.media_statuses
                .cancel_request(session_id, asset.id, &asset.path, request_id);
            false
        }
    }

    pub(crate) fn ensure_media_status(
        &mut self,
        asset: &MediaAsset,
    ) -> Option<MediaAvailabilityStatus> {
        let session_id = self.focused().id;
        if let Some(status) = self.media_statuses.status(session_id, asset) {
            return Some(status);
        }
        if let Some(request_id) = self.media_statuses.begin(session_id, asset) {
            let _ = self.spawn_media_status_request(session_id, asset, request_id);
        }
        None
    }

    /// Source imagery uses a time-bounded verified observation.  Once it
    /// expires this returns Checking immediately, starts exactly one async
    /// recheck, and gives the viewer a repaint deadline for the next expiry.
    pub(crate) fn source_media_status_for_asset(
        &mut self,
        asset: &MediaAsset,
    ) -> SourceMediaStatus {
        let session_id = self.focused().id;
        let snapshot = self
            .media_statuses
            .source_status_at(session_id, asset, Instant::now());
        if snapshot.status.is_none()
            && let Some(request_id) = self.media_statuses.begin(session_id, asset)
        {
            let _ = self.spawn_media_status_request(session_id, asset, request_id);
        }
        snapshot
    }

    /// Force an availability verification for a human Source edit. If the
    /// Source viewer already has the same full verification in flight, attach
    /// the edit to that generation rather than reading the whole file twice.
    /// The caller stores the captured edit context against the returned
    /// request id and dispatches only after that exact response is accepted.
    pub(crate) fn force_source_edit_media_revalidation(
        &mut self,
        asset: &MediaAsset,
    ) -> Option<u64> {
        let session_id = self.focused().id;
        if let Some(request_id) = self.media_statuses.pending_request_id(session_id, asset) {
            return Some(request_id);
        }
        let request_id = self.media_statuses.begin_forced(session_id, asset);
        self.spawn_media_status_request(session_id, asset, request_id)
            .then_some(request_id)
    }

    pub(crate) fn queue_media_status_checks_for_project(&mut self, project_index: usize) {
        let Some(project) = self.projects.get(project_index) else {
            return;
        };
        let assets = project.document.media_pool.clone();
        for asset in assets {
            let _ = self.ensure_media_status_for_project(project_index, &asset);
        }
    }

    pub(crate) fn refresh_media_statuses_for_focused_project(&mut self) {
        let session_id = self.focused().id;
        if cancel_pending_source_edit_for_session(&mut self.pending_source_edit, session_id) {
            self.record_error("Source monitor", SOURCE_EDIT_REFRESH_CANCELED_MESSAGE);
        }
        let assets = self
            .focused()
            .document
            .media_pool
            .iter()
            .map(|asset| (asset.id, asset.path.clone()))
            .collect::<Vec<_>>();
        for (asset_id, path) in assets {
            self.media_statuses.invalidate(session_id, asset_id);
            self.visual_cache.invalidate_path(&path);
        }
        self.queue_media_status_checks_for_project(self.focused_project);
    }

    fn ensure_media_status_for_project(
        &mut self,
        project_index: usize,
        asset: &MediaAsset,
    ) -> Option<MediaAvailabilityStatus> {
        let project = self.projects.get(project_index)?;
        let session_id = project.id;
        if let Some(status) = self.media_statuses.status(session_id, asset) {
            return Some(status);
        }
        if let Some(request_id) = self.media_statuses.begin(session_id, asset) {
            let _ = self.spawn_media_status_request(session_id, asset, request_id);
        }
        None
    }

    pub(crate) fn media_status_for_asset(
        &mut self,
        asset: &MediaAsset,
    ) -> Option<MediaAvailabilityStatus> {
        self.ensure_media_status(asset)
    }

    #[must_use]
    pub(crate) fn source_edit_revalidation_pending(&self) -> bool {
        self.pending_source_edit.is_some()
    }

    fn source_edit_context(&self, pending: &PendingSourceEdit) -> Option<SourceEditContext> {
        let project_index = session_index_by_id(pending.session_id, &self.projects)?;
        let session = &self.projects[project_index];
        let asset = session.document.asset(pending.asset_id)?;
        Some(SourceEditContext {
            session_id: session.id,
            asset_id: asset.id,
            path: asset.path.clone(),
            fingerprint: asset.source_fingerprint.clone(),
            revision: session.revision,
            selected_asset: session.selected_asset,
            source_position: session.source_position,
            timeline_in: session.position,
            source_in: session.source_in,
            source_out: session.source_out,
            video_target: session.source_video_target,
            audio_target: session.source_audio_target,
        })
    }

    fn reject_matching_pending_source_edit(
        &mut self,
        response: &MediaStatusResponse,
        message: &'static str,
    ) {
        if self
            .pending_source_edit
            .as_ref()
            .is_some_and(|pending| pending.matches_response(response))
        {
            self.pending_source_edit = None;
            self.record_error("Source monitor", message);
        }
    }

    fn complete_pending_source_edit_revalidation(&mut self, response: &MediaStatusResponse) {
        let current = self
            .pending_source_edit
            .as_ref()
            .and_then(|pending| self.source_edit_context(pending));
        match take_source_edit_dispatch_intent(
            &mut self.pending_source_edit,
            response,
            current.as_ref(),
        ) {
            Ok(None) => {}
            Err(rejection) => self.record_error("Source monitor", rejection.message()),
            Ok(Some(intent)) => {
                let Some(project_index) = session_index_by_id(intent.session_id, &self.projects)
                else {
                    self.record_error(
                        "Source monitor",
                        "Project session closed while Source was being verified",
                    );
                    return;
                };
                if self.projects[project_index]
                    .core
                    .send(Command::DoIfRevision {
                        expected: intent.expected_revision,
                        operation: intent.operation,
                    })
                    .is_err()
                {
                    self.record_error(
                        "Operations",
                        "Core actor stopped while applying patched source edit",
                    );
                } else {
                    self.status = format!(
                        "Applying verified Source patch at revision {}…",
                        intent.expected_revision
                    );
                }
            }
        }
    }

    fn handle_media_status_response(&mut self, response: &MediaStatusResponse) {
        if !self.media_statuses.accepts_response(
            response.session_id,
            response.asset_id,
            &response.path,
            response.request_id,
        ) {
            self.reject_matching_pending_source_edit(
                response,
                "Source verification was superseded before the edit could be applied",
            );
            return;
        }
        let Some(project_index) = session_index_by_id(response.session_id, &self.projects) else {
            self.reject_matching_pending_source_edit(
                response,
                "Project session closed while Source was being verified",
            );
            return;
        };
        let Some(asset) = self.projects[project_index]
            .document
            .asset(response.asset_id)
        else {
            self.reject_matching_pending_source_edit(
                response,
                "Selected source asset was removed while Source was being verified",
            );
            return;
        };
        if asset.path != response.path || asset.source_fingerprint != response.fingerprint {
            self.reject_matching_pending_source_edit(
                response,
                "Source file identity changed while Source was being verified",
            );
            return;
        }
        let blocked = source_display_state(Some(&response.status)).blocks_preview();
        let status_kind = response.status.kind;
        let invalidates_visuals = availability_invalidates_visuals(&response.status);
        let observed_path = response.path.clone();
        let accepted = self.media_statuses.finish(
            response.session_id,
            response.asset_id,
            response.request_id,
            response.path.clone(),
            response.fingerprint.clone(),
            response.status.clone(),
        );
        debug_assert!(accepted, "status route changed within one UI poll");
        if invalidates_visuals {
            self.visual_cache.block_path(&observed_path);
        } else if matches!(
            status_kind,
            MediaAvailabilityKind::OnlineVerified | MediaAvailabilityKind::OnlineUnverified
        ) && !self
            .media_statuses
            .path_has_changed_observation(&observed_path)
        {
            self.visual_cache
                .invalidate_and_unblock_path(&observed_path);
        }
        if blocked
            && project_index == self.focused_project
            && self.playhead_media_asset_id() == Some(response.asset_id)
        {
            self.texture = None;
            self.playback.pause();
            self.playing = false;
        }
        self.complete_pending_source_edit_revalidation(response);
    }

    fn handle_relink_probe_response(&mut self, response: RelinkProbeResponse) {
        self.relink_probe_pending = self.relink_probe_pending.saturating_sub(1);
        let Some(project_index) = session_index_by_id(response.session_id, &self.projects) else {
            return;
        };
        let project_name = self.projects[project_index].name.clone();
        if let Err(conflict) = validate_relink_revision(
            response.expected_revision,
            self.projects[project_index].revision,
        ) {
            self.record_error("Relink", conflict.message(&project_name));
            return;
        }
        let Some(target) = self.projects[project_index]
            .document
            .asset(response.asset_id)
            .cloned()
        else {
            return;
        };
        match response.result {
            Ok(probed) => {
                let candidate = build_relink_candidate(response.path, &probed);
                match preflight_relink(&target, &candidate) {
                    Ok(RelinkPreflight::Ready) => {
                        self.dispatch_relink(
                            project_index,
                            response.asset_id,
                            response.expected_revision,
                            candidate,
                            false,
                        );
                    }
                    Ok(RelinkPreflight::NeedsLegacyConfirmation) => {
                        self.pending_legacy_relink = Some(PendingLegacyRelink {
                            session_id: response.session_id,
                            asset_id: response.asset_id,
                            expected_revision: response.expected_revision,
                            candidate,
                            asset_name: target.name,
                        });
                    }
                    Err(rejection) => {
                        self.record_error("Relink", rejection.message(response.asset_id));
                    }
                }
            }
            Err(error) => self.record_error(
                "Relink",
                format!("Could not read replacement for {}: {error}", target.name),
            ),
        }
    }

    pub(crate) fn poll_media_workflow(&mut self, ctx: &egui::Context) {
        let mut changed = false;
        while let Ok(response) = self.media_status_rx.try_recv() {
            changed = true;
            self.handle_media_status_response(&response);
        }

        while let Ok(response) = self.relink_probe_rx.try_recv() {
            changed = true;
            self.handle_relink_probe_response(response);
        }

        while let Ok((family, result)) = self.cache_clear_rx.try_recv() {
            changed = true;
            self.media_cache_clear_pending = None;
            match result {
                Ok(result) => {
                    self.status = format!(
                        "Cleared {} ({} removed)",
                        cache_presentation(&MediaCacheFamilyStatus {
                            family: result.family,
                            supported: result.supported,
                            root: None,
                            file_count: result.removed_file_count,
                            bytes: result.removed_bytes,
                            may_repopulate: result.may_repopulate,
                            note: result.note.clone(),
                        })
                        .title,
                        format_bytes(result.removed_bytes)
                    );
                    self.media_cache_clear_result = Some(result);
                    self.media_cache_inventory = Some(self.analysis.cache_inventory());
                }
                Err(error) => self.record_error(
                    "Media cache",
                    format!("Could not clear {family:?} cache: {error}"),
                ),
            }
        }
        if changed {
            ctx.request_repaint();
        }
    }

    fn dispatch_relink(
        &mut self,
        project_index: usize,
        asset_id: AssetId,
        expected_revision: TimelineRevision,
        candidate: RelinkCandidate,
        allow_unverified_source: bool,
    ) {
        let project_name = self.projects[project_index].name.clone();
        if let Err(conflict) =
            validate_relink_revision(expected_revision, self.projects[project_index].revision)
        {
            self.record_error("Relink", conflict.message(&project_name));
            return;
        }
        let operation = Operation::RelinkAsset {
            asset: asset_id,
            candidate,
            allow_unverified_source,
        };
        if self.projects[project_index]
            .core
            .send(Command::DoIfRevision {
                expected: expected_revision,
                operation,
            })
            .is_err()
        {
            self.record_error(
                "Relink",
                format!("Core actor stopped while relinking in {project_name}"),
            );
        } else {
            self.status = format!("Relinking asset {asset_id}…");
        }
    }

    pub(crate) fn show_legacy_relink_confirmation(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_legacy_relink.clone() else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Confirm unverified relink")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(theme::caps_label(
                    "SOURCE IDENTITY UNKNOWN",
                    color::STATUS_WARNING,
                ));
                ui.label(format!(
                    "{} is a legacy asset without a stored source fingerprint.",
                    pending.asset_name
                ));
                ui.label("The replacement was probed successfully, but the original bytes cannot be verified.");
                ui.add_space(space::TWO);
                ui.label(format!("Use {}?", pending.candidate.path.display()));
                ui.add_space(space::TWO);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new("Relink without prior verification")
                                .fill(color::ACCENT_WASH)
                                .stroke(egui::Stroke::new(1.0, color::ACCENT_DIM_BORDER)),
                        )
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if confirm {
            self.pending_legacy_relink = None;
            if let Some(project_index) = session_index_by_id(pending.session_id, &self.projects) {
                let project_name = self.projects[project_index].name.clone();
                if let Err(conflict) = validate_relink_revision(
                    pending.expected_revision,
                    self.projects[project_index].revision,
                ) {
                    self.record_error("Relink", conflict.message(&project_name));
                    return;
                }
                let Some(target) = self.projects[project_index]
                    .document
                    .asset(pending.asset_id)
                    .cloned()
                else {
                    self.record_error("Relink", "The selected asset no longer exists");
                    return;
                };
                match preflight_relink(&target, &pending.candidate) {
                    Ok(RelinkPreflight::NeedsLegacyConfirmation) => self.dispatch_relink(
                        project_index,
                        pending.asset_id,
                        pending.expected_revision,
                        pending.candidate,
                        true,
                    ),
                    Ok(RelinkPreflight::Ready) => self.dispatch_relink(
                        project_index,
                        pending.asset_id,
                        pending.expected_revision,
                        pending.candidate,
                        false,
                    ),
                    Err(rejection) => {
                        self.record_error("Relink", rejection.message(pending.asset_id));
                    }
                }
            }
        } else if cancel {
            self.pending_legacy_relink = None;
            "Relink cancelled".clone_into(&mut self.status);
        }
    }

    pub(crate) fn open_media_cache_dialog(&mut self) {
        self.media_cache_dialog_open = true;
        self.media_cache_inventory = Some(self.analysis.cache_inventory());
        self.media_cache_clear_result = None;
    }

    fn clear_media_cache(&mut self, family: MediaCacheFamily) {
        if self.media_cache_clear_pending.is_some() {
            return;
        }
        let media = Arc::clone(&self.analysis);
        let result_tx = self.cache_clear_tx.clone();
        self.media_cache_clear_pending = Some(family);
        thread::Builder::new()
            .name("kinewright-cache-clear".to_owned())
            .spawn(move || {
                let result = media.clear_cache(family);
                let _ = result_tx.send((family, result));
            })
            .expect("failed to spawn cache clear worker");
    }

    pub(crate) fn show_media_cache_dialog(&mut self, ctx: &egui::Context) {
        if !self.media_cache_dialog_open {
            return;
        }
        let mut open = self.media_cache_dialog_open;
        let inventory = self.media_cache_inventory.clone();
        let pending = self.media_cache_clear_pending;
        let clear_result = self.media_cache_clear_result.clone();
        let mut requested_clear = None;
        let mut refresh = false;
        egui::Window::new("Media cache")
            .open(&mut open)
            .default_width(560.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label("Preview and derived-media storage");
                ui.colored_label(
                    color::TEXT_MUTED,
                    "These controls never change the project document or mark it dirty.",
                );
                ui.add_space(space::TWO);
                if let Some(inventory) = inventory {
                    for status in inventory.families {
                        let presentation = cache_presentation(&status);
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.strong(presentation.title);
                                ui.colored_label(color::TEXT_MUTED, presentation.persistence);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if presentation.clearable {
                                            let disabled = pending.is_some();
                                            if ui
                                                .add_enabled(!disabled, egui::Button::new("Clear"))
                                                .clicked()
                                            {
                                                requested_clear = Some(status.family);
                                            }
                                        } else {
                                            ui.colored_label(color::TEXT_MUTED, "Unsupported");
                                        }
                                    },
                                );
                            });
                            ui.colored_label(color::TEXT_MUTED, presentation.description);
                            if status.supported {
                                ui.colored_label(
                                    color::TEXT_MUTED,
                                    format!(
                                        "{} · {}",
                                        status.file_count,
                                        format_bytes(status.bytes)
                                    ),
                                );
                            } else if let Some(note) = &status.note {
                                ui.colored_label(color::TEXT_MUTED, note);
                            }
                        });
                        ui.add_space(space::ONE);
                    }
                } else {
                    ui.colored_label(color::TEXT_MUTED, "Reading cache inventory…");
                }
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        refresh = true;
                    }
                    if let Some(result) = &clear_result {
                        ui.colored_label(
                            color::STATUS_SUCCESS,
                            format!(
                                "Removed {} files ({})",
                                result.removed_file_count,
                                format_bytes(result.removed_bytes)
                            ),
                        );
                    }
                });
            });
        self.media_cache_dialog_open = open;
        if let Some(family) = requested_clear {
            self.clear_media_cache(family);
        }
        if refresh {
            self.media_cache_inventory = Some(self.analysis.cache_inventory());
        }
    }

    pub(crate) fn playhead_media_state(&mut self) -> Option<(SourceDisplayState, String)> {
        let position = self.focused().position;
        let document = Arc::clone(&self.focused().document);
        for track in &document.tracks {
            for clip in &track.clips {
                if !matches!(&clip.content, ClipContent::Media) {
                    continue;
                }
                let Ok(duration) = document.clip_duration(clip) else {
                    continue;
                };
                if position < clip.timeline_start
                    || position >= TimeCode(clip.timeline_start.0.saturating_add(duration.0))
                {
                    continue;
                }
                let Some(asset) = document.asset(clip.asset).cloned() else {
                    continue;
                };
                let status = self.media_status_for_asset(&asset);
                return Some((source_display_state(status.as_ref()), asset.name));
            }
        }
        None
    }

    pub(crate) fn playhead_media_asset_id(&self) -> Option<AssetId> {
        let position = self.focused().position;
        let document = &self.focused().document;
        for track in &document.tracks {
            for clip in &track.clips {
                if !matches!(&clip.content, ClipContent::Media) {
                    continue;
                }
                let Ok(duration) = document.clip_duration(clip) else {
                    continue;
                };
                if position >= clip.timeline_start
                    && position < TimeCode(clip.timeline_start.0.saturating_add(duration.0))
                {
                    return Some(clip.asset);
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// CC4 §7 look import, restore, and conversion
// ---------------------------------------------------------------------------

/// One finished `.cube` import, delivered from the worker thread.
///
/// Parsing, hashing, and the store write are the same worker-thread shape M41
/// established for the relink probe: a 65³ `.cube` is about 7.5 MB of text and
/// hashing it on the UI thread would stutter (CC4 §13).
#[derive(Debug)]
pub(crate) struct LutImportResponse {
    pub(crate) session_id: u64,
    pub(crate) path: PathBuf,
    /// What the caller wanted the imported bytes for, carried through the
    /// worker so a conversion cannot be mistaken for a plain import.
    pub(crate) intent: LutImportIntent,
    /// The store the worker actually wrote into, captured when the import
    /// started. A Save As that completes mid-import moves the session's root,
    /// so the handler has to compare and re-place the bytes (CC4 §2.2).
    pub(crate) store: LutStore,
    pub(crate) result: Result<LutAssetImport, MediaError>,
}

/// One finished `Locate file…` restore, delivered from the worker thread.
#[derive(Debug)]
pub(crate) struct LutRestoreResponse {
    pub(crate) session_id: u64,
    pub(crate) title: String,
    pub(crate) candidate: PathBuf,
    pub(crate) result: Result<PathBuf, MediaError>,
}

/// Why an import was started, which decides the batch its success submits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LutImportIntent {
    /// Register the asset and, when a video clip is selected, bind a new node
    /// of `stage` to it at the computed index.
    Apply {
        clip: Option<ClipId>,
        stage: ColorStage,
    },
    /// Retarget one existing LUT node onto the imported asset (`Replace…`).
    Retarget { clip: ClipId, effect: EffectId },
    /// Import the external `.cube` a legacy `cube_lut` names, then convert
    /// that node in the same undoable batch (CC4 §9).
    ConvertLegacy { clip: ClipId, effect: EffectId },
}

/// The `rfd` picker every look import goes through, matching `choose_media`
/// and `choose_relink_for_asset` (CC4 §7).
pub(crate) fn choose_lut_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Cube LUT", &["cube"])
        .pick_file()
}

/// Whether a dropped path is a `.cube`, so the window drop target can route it
/// to the look import instead of the media probe (CC4 §7).
pub(crate) fn is_cube_lut_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cube"))
}

/// The human message for a project that cannot own LUT bytes yet (CC4 §2.2).
pub(crate) const PROJECT_NOT_SAVED_MESSAGE: &str = "project_not_saved: save the project first so its looks have a \
     <stem>.kinewright-assets store to live in.";

/// Turn one successful import into the operations it submits (CC4 §7).
///
/// `AddLutAsset` always leads; a selected video clip additionally gets one
/// `InsertEffect` bound to the new asset at the first index that satisfies the
/// stage order, so a human can never author a `ColorStageOrderViolation`.
/// Emitted as one batch, so the import is one undo step.
pub(crate) fn lut_import_operations(
    document: &Document,
    import: LutAssetImport,
    intent: &LutImportIntent,
) -> Result<LutImportBatch, String> {
    let id = document
        .next_lut_asset_id()
        .map_err(|error| error.to_string())?;
    let asset = import.into_lut_asset(id);
    let mut batch = LutImportBatch {
        operations: vec![Operation::AddLutAsset {
            asset: asset.clone(),
        }],
        target_lost: None,
        asset,
    };
    match intent {
        LutImportIntent::Apply { clip: None, .. } => {}
        LutImportIntent::Apply {
            clip: Some(clip),
            stage,
        } => match document.clip(*clip) {
            Some(target) => batch
                .operations
                .push(crate::inspector_ui::insert_lut_node_operation(
                    target, *stage, id,
                )),
            None => batch.target_lost = Some(format!("clip {clip} no longer exists")),
        },
        LutImportIntent::Retarget { clip, effect } => {
            if lut_node_exists(document, *clip, *effect) {
                batch
                    .operations
                    .push(crate::inspector_ui::lut_asset_param_operation(
                        *clip, *effect, id,
                    ));
            } else {
                batch.target_lost =
                    Some(format!("effect {effect} on clip {clip} no longer exists"));
            }
        }
        LutImportIntent::ConvertLegacy { clip, effect } => {
            match document.clip(*clip).and_then(|target| {
                target
                    .effects
                    .iter()
                    .find(|node| node.id == *effect)
                    .map(|legacy| (target, legacy))
            }) {
                // Reuses the inspector's rule so a legacy stage that sits
                // before a managed correction is moved rather than rejected
                // (CC4 §3.2, §9).
                Some((target, legacy)) => {
                    batch
                        .operations
                        .extend(crate::inspector_ui::legacy_conversion_operations(
                            target, legacy, id,
                        ));
                }
                None => {
                    batch.target_lost =
                        Some(format!("effect {effect} on clip {clip} no longer exists"));
                }
            }
        }
    }
    Ok(batch)
}

/// What one successful import submits, and whether the node it was meant for
/// survived the round trip.
///
/// The bytes are already in the content-addressed store by the time the worker
/// answers, so a target that vanished mid-import degrades to registering the
/// asset alone rather than throwing the import away: the operator keeps the
/// look and binds it from the browser. The lost target is reported, never
/// silently substituted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LutImportBatch {
    pub(crate) operations: Vec<Operation>,
    pub(crate) target_lost: Option<String>,
    /// The record `AddLutAsset` registers, kept out of the operation list so
    /// the handler can reserve its id and place its bytes without pattern
    /// matching the batch back apart.
    pub(crate) asset: LutAsset,
}

/// The asset id one submitted import batch allocated, held until the document
/// records it (CC4 §7).
///
/// Two `.cube` files dropped together finish in the same frame. The batch is
/// built against the document the response lands on, and the second response
/// would otherwise be built against the same pre-import document as the first:
/// the same `next_lut_asset_id`, the same `next_effect_id`, and Core rejects
/// the second batch with `DuplicateLutAsset`, losing the import silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LutImportReservation {
    pub(crate) session_id: u64,
    pub(crate) asset: LutAssetId,
}

/// Whether the next queued import response may be built this frame.
///
/// `None` for the document means the reserved session is gone, which retires
/// the reservation: there is nothing left for the batch to collide with.
#[must_use]
pub(crate) fn lut_import_is_ready(
    reservation: Option<LutImportReservation>,
    document: Option<&Document>,
) -> bool {
    let Some(reservation) = reservation else {
        return true;
    };
    document.is_none_or(|document| document.lut_asset(reservation.asset).is_some())
}

/// Place freshly imported bytes in the store the session owns *now* (CC4 §2.2).
///
/// The worker captured its store when the import started; a Save As that
/// completed while it ran moved the root, so the bytes landed beside the old
/// project file and the new one would report the asset `missing`. Copying is
/// content-addressed and idempotent, and it is a no-op when the roots match.
///
/// # Errors
///
/// Returns the typed copy failure, so the import is refused loudly instead of
/// registering a record whose bytes are not where the project says they are.
pub(crate) fn realign_import_store(
    worker: &LutStore,
    current: &LutStore,
    asset: &LutAsset,
) -> Result<(), String> {
    if worker.root() == current.root() {
        return Ok(());
    }
    worker
        .copy_to(current, std::slice::from_ref(asset))
        .into_iter()
        .find_map(|(_, result)| result.err())
        .map_or(Ok(()), |error| Err(error.to_string()))
}

/// Why a finished import must be dropped instead of registered, or `None` when
/// its bytes are where the project now claims to own them (CC4 §2.2).
///
/// The `current` store being `None` is not "nothing to realign": it means a
/// Save As landed on a root this process refuses to use while the worker ran,
/// so the bytes are beside the *old* project file and the new root cannot
/// receive them. Registering the record anyway would publish an unreachable
/// asset under an "Imported look" status, which is exactly the silent
/// mis-report CC4 §2.3 exists to prevent.
#[must_use]
pub(crate) fn import_realignment_failure(
    title: &str,
    worker: &LutStore,
    current: Option<&LutStore>,
    store_unavailable: Option<&str>,
    asset: &LutAsset,
) -> Option<String> {
    let Some(current) = current else {
        return Some(format!(
            "Imported {title} into {}, but the project's look store is unavailable, so the bytes \
             could not be placed where the project owns them and the look was not registered: {}",
            worker.root().display(),
            store_unavailable.unwrap_or(PROJECT_NOT_SAVED_MESSAGE),
        ));
    };
    realign_import_store(worker, current, asset)
        .err()
        .map(|reason| {
            format!(
                "Imported {title} into {}, but the project was saved elsewhere while it \
                 imported and the bytes could not be moved to {}: {reason}",
                worker.root().display(),
                current.root().display()
            )
        })
}

/// Whether one effect is still on one clip.
fn lut_node_exists(document: &Document, clip: ClipId, effect: EffectId) -> bool {
    document
        .clip(clip)
        .is_some_and(|clip| clip.effects.iter().any(|node| node.id == effect))
}

/// The typed human message for one import or restore failure (CC4 §2.5, §7).
///
/// The store and parser errors already carry `code`, the offending 1-based
/// line, the observed fragment, and what was allowed; this only frames them
/// with the file they came from so the recovery action stays visible.
pub(crate) fn lut_failure_message(path: &Path, error: &MediaError) -> String {
    format!("{}: {error}", path.display())
}

impl KinewrightApp {
    /// `Replace…` on a missing or changed asset: import a different LUT and
    /// retarget the node that referenced the old one (CC4 §2.3).
    pub(crate) fn choose_lut_replacement(&mut self, clip: ClipId, effect: EffectId) {
        let Some(path) = choose_lut_file() else {
            return;
        };
        self.start_lut_import(path, LutImportIntent::Retarget { clip, effect });
    }

    /// Import the external `.cube` a legacy `cube_lut` names so the conversion
    /// batch has a registered asset to bind to (CC4 §9).
    pub(crate) fn start_legacy_cube_conversion(
        &mut self,
        clip: ClipId,
        effect: EffectId,
        path: PathBuf,
    ) {
        self.start_lut_import(path, LutImportIntent::ConvertLegacy { clip, effect });
    }

    /// Parse, hash, and store one `.cube` on a named worker thread (CC4 §7).
    pub(crate) fn start_lut_import(&mut self, path: PathBuf, intent: LutImportIntent) {
        let Some(store) = self.focused().lut_store.clone() else {
            // A refused root reports why it was refused; only a project that
            // has never been saved reports the save recovery (CC4 §2.2).
            self.record_error(
                "Look",
                self.focused()
                    .lut_store_unavailable_reason()
                    .unwrap_or_else(|| PROJECT_NOT_SAVED_MESSAGE.to_owned()),
            );
            return;
        };
        let session_id = self.focused().id;
        let result_tx = self.lut_import_tx.clone();
        self.lut_worker_pending = self.lut_worker_pending.saturating_add(1);
        self.status = format!("Importing look {}…", path.display());
        let spawn = thread::Builder::new()
            .name("kinewright-lut-import".to_owned())
            .spawn(move || {
                let result = store.import_lut_asset(&path);
                let _ = result_tx.send(LutImportResponse {
                    session_id,
                    path,
                    intent,
                    store,
                    result,
                });
            });
        if let Err(error) = spawn {
            self.lut_worker_pending = self.lut_worker_pending.saturating_sub(1);
            self.record_error("Look", format!("Could not start the look import: {error}"));
        }
    }

    /// `Locate file…`: pick a candidate and hash-check it into the store.
    pub(crate) fn choose_lut_restore(&mut self, lut_asset: LutAssetId) {
        let Some(asset) = self.focused().document.lut_asset(lut_asset).cloned() else {
            self.record_error("Look", format!("LUT asset {lut_asset} no longer exists"));
            return;
        };
        let Some(candidate) = choose_lut_file() else {
            return;
        };
        self.start_lut_restore(&asset, candidate);
    }

    /// Restore one asset's bytes from a candidate file (CC4 §2.3).
    ///
    /// A media repair action, not a Core operation: content addressing means
    /// the document stores no locator to change, so a restore changes no
    /// document state, does not dirty the project, and needs no revision gate.
    pub(crate) fn start_lut_restore(&mut self, asset: &LutAsset, candidate: PathBuf) {
        let Some(store) = self.focused().lut_store.clone() else {
            self.record_error(
                "Look",
                self.focused()
                    .lut_store_unavailable_reason()
                    .unwrap_or_else(|| PROJECT_NOT_SAVED_MESSAGE.to_owned()),
            );
            return;
        };
        let session_id = self.focused().id;
        let title = asset.title.clone();
        let asset = asset.clone();
        let result_tx = self.lut_restore_tx.clone();
        self.lut_worker_pending = self.lut_worker_pending.saturating_add(1);
        self.status = format!("Locating look {title}…");
        let spawn = thread::Builder::new()
            .name("kinewright-lut-restore".to_owned())
            .spawn(move || {
                let result = store.restore(&asset, &candidate);
                let _ = result_tx.send(LutRestoreResponse {
                    session_id,
                    title,
                    candidate,
                    result,
                });
            });
        if let Err(error) = spawn {
            self.lut_worker_pending = self.lut_worker_pending.saturating_sub(1);
            self.record_error("Look", format!("Could not start the look restore: {error}"));
        }
    }

    fn handle_lut_import_response(&mut self, response: LutImportResponse) {
        self.lut_worker_pending = self.lut_worker_pending.saturating_sub(1);
        let Some(project_index) = session_index_by_id(response.session_id, &self.projects) else {
            return;
        };
        let import = match response.result {
            Ok(import) => import,
            Err(error) => {
                self.record_error("Look", lut_failure_message(&response.path, &error));
                return;
            }
        };
        let title = import.title.clone();
        // Built against the *current* document, not the revision the picker
        // was opened at: an import creates a new record, so its id and its
        // insert index must come from the document the batch will land on.
        // Core still validates the batch atomically, so a concurrent change
        // that invalidates it is rejected rather than silently applied.
        let batch = match lut_import_operations(
            &self.projects[project_index].document,
            import,
            &response.intent,
        ) {
            Ok(batch) => batch,
            Err(error) => {
                self.record_error("Look", error);
                return;
            }
        };
        // A Save As that completed while the worker ran moved this session's
        // store root, so the bytes are beside the *old* project file. Place
        // them where the project now claims to own them before the document
        // records a record that resolves against the new root (CC4 §2.2).
        //
        // A store that is `None` is the same failure, not an exemption: the
        // Save As landed on a refused root, so there is nowhere to put the
        // bytes and the batch is dropped with the store's own reason rather
        // than registering an unreachable asset as an imported look.
        let store_unavailable = self.projects[project_index].lut_store_unavailable_reason();
        let realignment = import_realignment_failure(
            &title,
            &response.store,
            self.projects[project_index].lut_store.as_ref(),
            store_unavailable.as_deref(),
            &batch.asset,
        );
        if let Some(message) = realignment {
            self.record_error("Look", message);
            return;
        }
        if self.projects[project_index]
            .core
            .send(Command::DoBatch(batch.operations))
            .is_err()
        {
            self.record_error("Look", "Core actor stopped while registering the look");
            return;
        }
        // Hold the allocated id until the document records it, so a second
        // response finishing in the same frame cannot be built against the
        // same pre-import document (CC4 §7).
        self.lut_import_reservation = Some(LutImportReservation {
            session_id: response.session_id,
            asset: batch.asset.id,
        });
        if let Some(lost) = batch.target_lost {
            self.record_error(
                "Look",
                format!(
                    "Imported {title}, but {lost}, so it was registered without being applied. \
                     Apply it from the look browser."
                ),
            );
        }
        self.status = format!("Imported look {title}");
    }

    fn handle_lut_restore_response(&mut self, response: LutRestoreResponse) {
        self.lut_worker_pending = self.lut_worker_pending.saturating_sub(1);
        let Some(project_index) = session_index_by_id(response.session_id, &self.projects) else {
            return;
        };
        match response.result {
            Ok(_) => {
                // The bytes changed under a document that did not, so the
                // library has to be rebuilt explicitly: nothing else observes
                // a restore.
                let library = self.projects[project_index].rebuild_lut_library();
                if project_index == self.focused_project {
                    self.lut_publisher.set_lut_library(library);
                }
                self.status = format!("Restored look {}", response.title);
            }
            Err(error) => self.record_error(
                "Look",
                format!(
                    "Could not restore {}: {}",
                    response.title,
                    lut_failure_message(&response.candidate, &error)
                ),
            ),
        }
    }

    /// Drain the look worker channels once per frame.
    ///
    /// At most one *import* is taken per frame, and only once the previous
    /// import's asset id has landed in the document: an import batch is built
    /// against the document it will be applied to, so two of them built from
    /// the same document would allocate the same `next_lut_asset_id` and the
    /// second would be rejected `DuplicateLutAsset`. The rest stay in the
    /// channel with `lut_worker_pending` still counting them, which keeps the
    /// frame loop repainting until every one has landed (CC4 §7).
    pub(crate) fn poll_lut_workers(&mut self) -> bool {
        let mut changed = false;
        let reserved_document = self.lut_import_reservation.and_then(|reservation| {
            session_index_by_id(reservation.session_id, &self.projects)
                .map(|index| Arc::clone(&self.projects[index].document))
        });
        if lut_import_is_ready(self.lut_import_reservation, reserved_document.as_deref()) {
            self.lut_import_reservation = None;
            if let Ok(response) = self.lut_import_rx.try_recv() {
                changed = true;
                self.handle_lut_import_response(response);
            }
        }
        while let Ok(response) = self.lut_restore_rx.try_recv() {
            changed = true;
            self.handle_lut_restore_response(response);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use kinewright_core::{ColorDescription, MediaKind, Rational, TimeCode};
    use kinewright_media::test_support::TempDirectory;

    use super::*;

    fn fingerprint(hash: &str, bytes: u64) -> MediaSourceFingerprint {
        MediaSourceFingerprint {
            content_sha256: Some(hash.to_owned()),
            byte_len: Some(bytes),
        }
    }

    fn asset(fingerprint: MediaSourceFingerprint) -> MediaAsset {
        MediaAsset {
            id: AssetId(7),
            path: PathBuf::from("original.mov"),
            name: "original.mov".to_owned(),
            duration: TimeCode(120),
            fps: Rational::new(24, 1).expect("valid fps"),
            kind: MediaKind::Video,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: fingerprint,
            color_description: ColorDescription::default(),
        }
    }

    fn candidate(fingerprint: MediaSourceFingerprint) -> RelinkCandidate {
        RelinkCandidate {
            path: PathBuf::from("replacement.mov"),
            fingerprint,
            kind: MediaKind::Video,
            fps: Rational::new(24, 1).expect("valid fps"),
            duration: TimeCode(120),
            resolution: Some((1_920, 1_080)),
        }
    }

    fn status(kind: MediaAvailabilityKind) -> MediaAvailabilityStatus {
        MediaAvailabilityStatus {
            kind,
            observed_fingerprint: None,
            reason: None,
        }
    }

    fn pending_source_edit(request_id: u64) -> PendingSourceEdit {
        let source = asset(fingerprint(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            4,
        ));
        PendingSourceEdit {
            session_id: 3,
            request_id,
            asset_id: source.id,
            path: source.path,
            fingerprint: source.source_fingerprint,
            expected_revision: TimelineRevision(11),
            selected_asset: Some(source.id),
            source_position: TimeCode(7),
            timeline_in: TimeCode(19),
            source_in: TimeCode(4),
            source_out: TimeCode(12),
            video_target: Some(TrackId(1)),
            audio_target: Some(TrackId(2)),
            mode: ThreePointMode::Insert,
        }
    }

    fn edit_response(
        pending: &PendingSourceEdit,
        kind: MediaAvailabilityKind,
    ) -> MediaStatusResponse {
        MediaStatusResponse {
            session_id: pending.session_id,
            asset_id: pending.asset_id,
            request_id: pending.request_id,
            path: pending.path.clone(),
            fingerprint: pending.fingerprint.clone(),
            status: status(kind),
        }
    }

    fn edit_context(pending: &PendingSourceEdit) -> SourceEditContext {
        SourceEditContext {
            session_id: pending.session_id,
            asset_id: pending.asset_id,
            path: pending.path.clone(),
            fingerprint: pending.fingerprint.clone(),
            revision: pending.expected_revision,
            selected_asset: pending.selected_asset,
            source_position: pending.source_position,
            timeline_in: pending.timeline_in,
            source_in: pending.source_in,
            source_out: pending.source_out,
            video_target: pending.video_target,
            audio_target: pending.audio_target,
        }
    }

    #[test]
    fn source_labels_keep_failure_states_visible() {
        assert_eq!(
            SourceDisplayState::OnlineVerified.label(),
            "ONLINE · VERIFIED"
        );
        assert_eq!(
            SourceDisplayState::OnlineUnverified.label(),
            "ONLINE · UNVERIFIED"
        );
        assert_eq!(SourceDisplayState::Offline.label(), "OFFLINE");
        assert_eq!(SourceDisplayState::Changed.label(), "CHANGED");
        assert_eq!(SourceDisplayState::Unreadable.label(), "UNREADABLE");
        assert!(SourceDisplayState::Offline.blocks_preview());
        assert!(SourceDisplayState::Changed.is_warning());
    }

    #[test]
    fn source_access_and_edit_eligibility_require_verified_live_status() {
        let unavailable = [
            SourceDisplayState::Checking,
            SourceDisplayState::OnlineUnverified,
            SourceDisplayState::Offline,
            SourceDisplayState::Changed,
            SourceDisplayState::Unreadable,
        ];
        assert_eq!(source_display_state(None), SourceDisplayState::Checking);
        for state in unavailable {
            assert!(!state.allows_verified_source_access(), "{state:?}");
            assert!(
                !source_edit_is_eligible(state, 120, 0, 24, true),
                "{state:?}"
            );
        }
        assert!(SourceDisplayState::OnlineVerified.allows_verified_source_access());
        assert!(source_edit_is_eligible(
            SourceDisplayState::OnlineVerified,
            120,
            0,
            24,
            true,
        ));
        assert!(!source_edit_is_eligible(
            SourceDisplayState::OnlineVerified,
            120,
            24,
            24,
            true,
        ));
        assert!(!source_edit_is_eligible(
            SourceDisplayState::OnlineVerified,
            120,
            0,
            24,
            false,
        ));
        assert!(source_access_is_allowed(
            SourceDisplayState::OnlineVerified,
            false,
        ));
        assert!(!source_access_is_allowed(
            SourceDisplayState::OnlineVerified,
            true,
        ));
        assert!(!source_edit_controls_are_enabled(
            SourceDisplayState::OnlineVerified,
            120,
            0,
            24,
            true,
            true,
        ));
    }

    #[test]
    fn source_texture_gate_never_reuses_pixels_without_verified_observation() {
        assert!(should_display_source_texture(
            SourceDisplayState::OnlineVerified,
            true,
        ));
        assert!(!should_display_source_texture(
            SourceDisplayState::OnlineVerified,
            false,
        ));
        for state in [
            SourceDisplayState::Checking,
            SourceDisplayState::OnlineUnverified,
            SourceDisplayState::Offline,
            SourceDisplayState::Changed,
            SourceDisplayState::Unreadable,
        ] {
            assert!(!should_display_source_texture(state, true), "{state:?}");
        }
    }

    #[test]
    fn verified_source_observation_expires_then_starts_only_one_recheck() {
        let mut store = MediaStatusStore::default();
        let source = asset(fingerprint(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            4,
        ));
        let request = store.begin(3, &source).expect("initial request starts");
        let observed_at = Instant::now();
        assert!(store.finish_at(
            3,
            source.id,
            request,
            source.path.clone(),
            source.source_fingerprint.clone(),
            status(MediaAvailabilityKind::OnlineVerified),
            observed_at,
        ));

        let fresh = store.source_status_at(
            3,
            &source,
            observed_at + Duration::from_secs(SOURCE_VERIFIED_FRESHNESS.as_secs() - 1),
        );
        assert_eq!(
            fresh.status.map(|status| status.kind),
            Some(MediaAvailabilityKind::OnlineVerified)
        );
        assert_eq!(fresh.refresh_after, Some(Duration::from_secs(1)));

        let expired = store.source_status_at(3, &source, observed_at + SOURCE_VERIFIED_FRESHNESS);
        assert!(expired.status.is_none());
        assert!(store.status(3, &source).is_none());
        let recheck = store.begin(3, &source).expect("expired status rechecks");
        assert_eq!(store.pending_request_id(3, &source), Some(recheck));
        assert!(store.begin(3, &source).is_none());
    }

    #[test]
    fn forced_source_revalidation_reuses_inflight_generation_and_drops_late_ones() {
        let mut store = MediaStatusStore::default();
        let source = asset(fingerprint(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            4,
        ));
        let first = store.begin(3, &source).expect("initial status starts");
        assert_eq!(store.begin_forced(3, &source), first);
        assert!(store.accepts_response(3, source.id, &source.path, first));
        assert!(store.finish(
            3,
            source.id,
            first,
            source.path.clone(),
            source.source_fingerprint.clone(),
            status(MediaAvailabilityKind::OnlineVerified),
        ));

        let second = store.begin_forced(3, &source);
        assert_ne!(second, first);
        assert!(!store.accepts_response(3, source.id, &source.path, first));
        assert!(store.accepts_response(3, source.id, &source.path, second));
    }

    #[test]
    fn source_edit_dispatch_requires_verified_response_and_current_context() {
        let pending = pending_source_edit(41);
        let verified = edit_response(&pending, MediaAvailabilityKind::OnlineVerified);
        let current = edit_context(&pending);
        assert!(source_edit_dispatch_intent(&pending, &verified, Some(&current)).is_ok());

        let changed = edit_response(&pending, MediaAvailabilityKind::Changed);
        assert_eq!(
            source_edit_dispatch_intent(&pending, &changed, Some(&current)),
            Err(SourceEditRejection::SourceNoLongerVerified)
        );

        let mut revision_drift = current.clone();
        revision_drift.revision = TimelineRevision(12);
        assert_eq!(
            source_edit_dispatch_intent(&pending, &verified, Some(&revision_drift)),
            Err(SourceEditRejection::RevisionChanged)
        );

        let mut marks_drift = current.clone();
        marks_drift.source_out = TimeCode(13);
        assert_eq!(
            source_edit_dispatch_intent(&pending, &verified, Some(&marks_drift)),
            Err(SourceEditRejection::SourceMarksChanged)
        );

        let mut routes_drift = current.clone();
        routes_drift.audio_target = None;
        assert_eq!(
            source_edit_dispatch_intent(&pending, &verified, Some(&routes_drift)),
            Err(SourceEditRejection::SourceRoutesChanged)
        );

        let mut selection_drift = current;
        selection_drift.selected_asset = Some(AssetId(99));
        assert_eq!(
            source_edit_dispatch_intent(&pending, &verified, Some(&selection_drift)),
            Err(SourceEditRejection::SourceSelectionChanged)
        );
    }

    #[test]
    fn late_generation_cannot_consume_a_newer_pending_source_edit() {
        let pending = pending_source_edit(52);
        let mut slot = Some(pending.clone());
        let mut late = edit_response(&pending, MediaAvailabilityKind::OnlineVerified);
        late.request_id = 51;
        assert_eq!(
            take_source_edit_dispatch_intent(&mut slot, &late, Some(&edit_context(&pending))),
            Ok(None)
        );
        assert_eq!(slot, Some(pending));
    }

    #[test]
    fn completed_source_edit_response_yields_exactly_one_dispatch_intent() {
        let pending = pending_source_edit(61);
        let response = edit_response(&pending, MediaAvailabilityKind::OnlineVerified);
        let current = edit_context(&pending);
        let mut slot = Some(pending);
        let first = take_source_edit_dispatch_intent(&mut slot, &response, Some(&current))
            .expect("verified response must be processable");
        let second = take_source_edit_dispatch_intent(&mut slot, &response, Some(&current))
            .expect("duplicate response must be harmless");
        assert!(first.is_some());
        assert!(second.is_none());
    }

    #[test]
    fn refresh_cancels_pending_source_edit_before_replacing_status_generation() {
        let source = asset(fingerprint(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            4,
        ));
        let mut statuses = MediaStatusStore::default();
        let first_request = statuses.begin(3, &source).expect("initial request starts");
        let pending = pending_source_edit(first_request);
        let mut pending_slot = Some(pending.clone());

        assert!(cancel_pending_source_edit_for_session(&mut pending_slot, 3));
        assert!(
            pending_slot.is_none(),
            "Refresh must unlock Source controls"
        );

        statuses.invalidate(3, source.id);
        let second_request = statuses
            .begin(3, &source)
            .expect("refresh requeues request");
        assert_ne!(second_request, first_request);
        assert!(!statuses.accepts_response(3, source.id, &source.path, first_request));
        assert!(statuses.accepts_response(3, source.id, &source.path, second_request));

        let old_response = edit_response(&pending, MediaAvailabilityKind::OnlineVerified);
        let mut new_response = old_response.clone();
        new_response.request_id = second_request;
        assert!(
            take_source_edit_dispatch_intent(&mut pending_slot, &old_response, None)
                .expect("late old response is harmless")
                .is_none()
        );
        assert!(
            take_source_edit_dispatch_intent(&mut pending_slot, &new_response, None)
                .expect("new response cannot resurrect a canceled edit")
                .is_none()
        );
    }

    #[test]
    fn candidate_uses_target_path_and_probe_metadata_without_probe_id() {
        let probed = asset(fingerprint(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            4,
        ));
        let candidate = build_relink_candidate(PathBuf::from("chosen.mov"), &probed);
        assert_eq!(candidate.path, PathBuf::from("chosen.mov"));
        assert_eq!(candidate.fingerprint, probed.source_fingerprint);
        assert_eq!(candidate.kind, probed.kind);
        assert_eq!(candidate.duration, probed.duration);
    }

    #[test]
    fn known_identity_mismatch_is_blocked_before_operation_dispatch() {
        let target = asset(fingerprint(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            4,
        ));
        let replacement = candidate(fingerprint(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            4,
        ));
        assert!(matches!(
            preflight_relink(&target, &replacement),
            Err(RelinkRejection::FingerprintMismatch { .. })
        ));
    }

    #[test]
    fn legacy_identity_requires_confirmation() {
        let target = asset(MediaSourceFingerprint::unknown());
        let replacement = candidate(fingerprint(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            4,
        ));
        assert_eq!(
            preflight_relink(&target, &replacement),
            Ok(RelinkPreflight::NeedsLegacyConfirmation)
        );
    }

    #[test]
    fn probe_response_preserves_the_revision_captured_before_async_work() {
        let expected_revision = TimelineRevision(17);
        let response = RelinkProbeResponse {
            session_id: 3,
            asset_id: AssetId(7),
            expected_revision,
            path: PathBuf::from("replacement.mov"),
            result: Ok(asset(MediaSourceFingerprint::unknown())),
        };

        assert_eq!(response.expected_revision, expected_revision);
        assert_eq!(response.session_id, 3);
    }

    #[test]
    fn pending_legacy_confirmation_preserves_probe_revision() {
        let expected_revision = TimelineRevision(23);
        let pending = PendingLegacyRelink {
            session_id: 3,
            asset_id: AssetId(7),
            expected_revision,
            candidate: candidate(fingerprint(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                4,
            )),
            asset_name: "original.mov".to_owned(),
        };

        assert_eq!(pending.expected_revision, expected_revision);
    }

    #[test]
    fn relink_revision_gate_rejects_stale_async_results() {
        assert_eq!(
            validate_relink_revision(TimelineRevision(8), TimelineRevision(8)),
            Ok(())
        );
        let conflict = validate_relink_revision(TimelineRevision(8), TimelineRevision(9))
            .expect_err("a changed project must reject the relink");
        assert_eq!(conflict.expected, TimelineRevision(8));
        assert_eq!(conflict.actual, TimelineRevision(9));
        let message = conflict.message("Demo");
        assert!(message.contains("expected timeline revision 8"));
        assert!(message.contains("current revision is 9"));
    }

    #[test]
    fn only_changed_availability_invalidates_path_keyed_visuals() {
        assert!(availability_invalidates_visuals(&status(
            MediaAvailabilityKind::Changed
        )));
        assert!(!availability_invalidates_visuals(&status(
            MediaAvailabilityKind::OnlineVerified
        )));
        assert!(!availability_invalidates_visuals(&status(
            MediaAvailabilityKind::OfflineMissing
        )));
    }

    #[test]
    fn refreshed_status_request_rejects_an_older_async_response() {
        let mut store = MediaStatusStore::default();
        let source = asset(fingerprint(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            4,
        ));
        let first_request = store.begin(3, &source).expect("first request starts");
        store.invalidate(3, source.id);
        let refreshed_request = store
            .begin(3, &source)
            .expect("refresh starts a new request");

        assert_ne!(first_request, refreshed_request);
        assert!(!store.accepts_response(3, source.id, &source.path, first_request));
        assert!(store.accepts_response(3, source.id, &source.path, refreshed_request));
        assert!(!store.finish(
            3,
            source.id,
            first_request,
            source.path.clone(),
            source.source_fingerprint.clone(),
            status(MediaAvailabilityKind::OnlineVerified),
        ));
        assert!(store.status(3, &source).is_none());
        assert!(store.finish(
            3,
            source.id,
            refreshed_request,
            source.path.clone(),
            source.source_fingerprint.clone(),
            status(MediaAvailabilityKind::Changed),
        ));
        assert_eq!(
            store.status(3, &source).map(|status| status.kind),
            Some(MediaAvailabilityKind::Changed)
        );
        assert!(store.path_has_changed_observation(&source.path));
    }

    #[test]
    fn successful_same_identity_relink_still_refreshes_machine_local_media_state() {
        let current = asset(fingerprint(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            4,
        ));
        let mut relink_candidate = candidate(current.source_fingerprint.clone());
        relink_candidate.path.clone_from(&current.path);
        let operation = Operation::RelinkAsset {
            asset: current.id,
            candidate: relink_candidate,
            allow_unverified_source: false,
        };

        assert!(!media_asset_requires_refresh(
            Some(&current),
            &current,
            None
        ));
        assert!(media_asset_requires_refresh(
            Some(&current),
            &current,
            Some(&operation)
        ));
    }

    #[test]
    fn cache_presentation_distinguishes_memory_derived_and_unsupported() {
        let family = |family, supported| MediaCacheFamilyStatus {
            family,
            supported,
            root: None,
            file_count: 0,
            bytes: 0,
            may_repopulate: true,
            note: None,
        };
        let preview = cache_presentation(&family(MediaCacheFamily::PreviewMemory, true));
        assert!(preview.persistence.contains("Ephemeral"));
        assert!(preview.description.contains("no generated proxy file"));
        let derived = cache_presentation(&family(MediaCacheFamily::DerivedAnalysis, true));
        assert!(derived.persistence.contains("Persistent"));
        let proxy = cache_presentation(&family(MediaCacheFamily::GeneratedProxy, false));
        assert!(!proxy.clearable);
        assert!(proxy.persistence.contains("Unsupported"));
    }

    // -----------------------------------------------------------------------
    // CC4 §7 look import batches
    // -----------------------------------------------------------------------

    fn sample_import() -> LutAssetImport {
        LutAssetImport {
            // A syntactically valid digest; the store is what proves content,
            // and this test never touches the filesystem.
            sha256: "a".repeat(64),
            title: "Fixture look".to_owned(),
            kind: kinewright_core::LutAssetKind::Cube3d,
            size: 2,
            byte_len: 256,
            domain_min_millionths: [0, 0, 0],
            domain_max_millionths: [1_000_000, 1_000_000, 1_000_000],
            source_path: "/looks/fixture.cube".to_owned(),
        }
    }

    fn import_document(effects: Vec<kinewright_core::Effect>) -> Document {
        let fps = Rational::new(24, 1).expect("valid fps");
        Document {
            tracks: vec![kinewright_core::Track {
                id: TrackId(1),
                kind: kinewright_core::TrackKind::Video,
                sync_lock: true,
                clips: vec![kinewright_core::Clip {
                    id: kinewright_core::ClipId(10),
                    asset: AssetId(1),
                    timeline_start: TimeCode::ZERO,
                    source_range: TimeCode::ZERO..TimeCode(24),
                    content: ClipContent::Media,
                    effects,
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: PathBuf::from("shot.mov"),
                name: "Shot".to_owned(),
                duration: TimeCode(24),
                fps,
                kind: MediaKind::Video,
                resolution: Some((1920, 1080)),
                source_fingerprint: MediaSourceFingerprint::unknown(),
                color_description: ColorDescription::default(),
            }],
            fps,
            resolution: (1920, 1080),
            duration: TimeCode(24),
            ..Document::default()
        }
    }

    fn effect(id: u64, name: &str) -> kinewright_core::Effect {
        kinewright_core::Effect {
            id: EffectId(id),
            name: name.to_owned(),
            parameters: std::collections::BTreeMap::new(),
            keyframes: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn only_a_cube_extension_routes_a_drop_to_the_look_import() {
        assert!(is_cube_lut_path(Path::new("/looks/k2383.cube")));
        // Case-insensitive, because a vendor export is often `.CUBE`.
        assert!(is_cube_lut_path(Path::new("/looks/K2383.CUBE")));
        assert!(!is_cube_lut_path(Path::new("/media/shot.mov")));
        assert!(!is_cube_lut_path(Path::new("/looks/cube")));
    }

    #[test]
    fn an_import_with_a_selected_clip_registers_and_inserts_as_one_batch() {
        let document = import_document(vec![effect(1, "primary_correction")]);
        let batch = lut_import_operations(
            &document,
            sample_import(),
            &LutImportIntent::Apply {
                clip: Some(ClipId(10)),
                stage: ColorStage::Look,
            },
        )
        .expect("the batch builds");

        assert_eq!(batch.operations.len(), 2);
        assert!(batch.target_lost.is_none());
        let Operation::AddLutAsset { asset } = &batch.operations[0] else {
            panic!("an import always leads with AddLutAsset");
        };
        assert_eq!(asset.id, LutAssetId(1));
        assert_eq!(asset.title, "Fixture look");
        assert!(matches!(
            asset.source,
            kinewright_core::LutAssetSource::Imported { .. }
        ));
        // The look lands after the correction, which is the first index that
        // satisfies the stage order.
        assert!(matches!(
            batch.operations[1],
            Operation::InsertEffect { index: 1, .. }
        ));
    }

    #[test]
    fn an_import_with_no_clip_registers_the_asset_alone() {
        let document = import_document(Vec::new());
        let batch = lut_import_operations(
            &document,
            sample_import(),
            &LutImportIntent::Apply {
                clip: None,
                stage: ColorStage::Look,
            },
        )
        .expect("the batch builds");
        assert_eq!(batch.operations.len(), 1);
        assert!(matches!(batch.operations[0], Operation::AddLutAsset { .. }));
        assert!(batch.target_lost.is_none());
    }

    #[test]
    fn a_replace_registers_the_new_asset_and_retargets_the_node() {
        // CC4 §2.3: a different LUT is a *different asset*. Nothing rewrites a
        // hash in place.
        let document = import_document(vec![effect(4, "creative_look")]);
        let batch = lut_import_operations(
            &document,
            sample_import(),
            &LutImportIntent::Retarget {
                clip: ClipId(10),
                effect: EffectId(4),
            },
        )
        .expect("the batch builds");
        assert_eq!(batch.operations.len(), 2);
        assert!(matches!(batch.operations[0], Operation::AddLutAsset { .. }));
        assert_eq!(
            batch.operations[1],
            Operation::SetEffectParam {
                clip: ClipId(10),
                effect: EffectId(4),
                name: kinewright_core::LUT_ASSET_ID_PARAMETER.to_owned(),
                value: kinewright_core::ParamValue::Integer(1),
            }
        );
    }

    #[test]
    fn a_legacy_cube_conversion_registers_then_converts() {
        let mut legacy = effect(4, "cube_lut");
        legacy.parameters.insert(
            "intensity_percent".to_owned(),
            kinewright_core::ParamValue::Integer(40),
        );
        let document = import_document(vec![legacy]);
        let batch = lut_import_operations(
            &document,
            sample_import(),
            &LutImportIntent::ConvertLegacy {
                clip: ClipId(10),
                effect: EffectId(4),
            },
        )
        .expect("the batch builds");
        assert_eq!(batch.operations.len(), 2);
        assert!(matches!(batch.operations[0], Operation::AddLutAsset { .. }));
        assert_eq!(
            batch.operations[1],
            Operation::ConvertLegacyLook {
                clip: ClipId(10),
                effect: EffectId(4),
                lut_asset: LutAssetId(1),
                mix_basis_points: 4_000,
            }
        );
    }

    /// The import worker's conversion path shares the inspector's ordering
    /// rule, so a legacy stage before a managed correction is moved rather
    /// than rejected (CC4 §3.2, §9).
    #[test]
    fn a_legacy_cube_conversion_before_a_correction_reorders_it() {
        let document =
            import_document(vec![effect(4, "cube_lut"), effect(1, "primary_correction")]);
        let batch = lut_import_operations(
            &document,
            sample_import(),
            &LutImportIntent::ConvertLegacy {
                clip: ClipId(10),
                effect: EffectId(4),
            },
        )
        .expect("the batch builds");
        assert_eq!(batch.operations.len(), 3);
        assert!(matches!(
            batch.operations[1],
            Operation::RemoveEffect {
                effect: EffectId(4),
                ..
            }
        ));
        assert!(matches!(
            batch.operations[2],
            Operation::InsertEffect { index: 1, .. }
        ));
    }

    #[test]
    fn a_target_that_vanished_mid_import_still_registers_the_bytes() {
        // The store write already happened, so throwing the whole batch away
        // would lose the operator's import. The asset is registered and the
        // lost target is reported, never silently substituted.
        let document = import_document(Vec::new());
        for intent in [
            LutImportIntent::Apply {
                clip: Some(ClipId(99)),
                stage: ColorStage::Look,
            },
            LutImportIntent::Retarget {
                clip: ClipId(10),
                effect: EffectId(99),
            },
            LutImportIntent::ConvertLegacy {
                clip: ClipId(10),
                effect: EffectId(99),
            },
        ] {
            let batch = lut_import_operations(&document, sample_import(), &intent)
                .unwrap_or_else(|error| panic!("{intent:?} did not build: {error}"));
            assert_eq!(batch.operations.len(), 1, "{intent:?}");
            assert!(matches!(batch.operations[0], Operation::AddLutAsset { .. }));
            assert!(batch.target_lost.is_some(), "{intent:?}");
        }
    }

    /// CC4 §7: two `.cube` files dropped together finish in the same frame.
    /// Both must register, under distinct ids. Draining the channel in one
    /// pass builds the second batch against the same pre-import document as
    /// the first, which Core rejects — the operator's second look vanishes
    /// with only an error-log line.
    #[test]
    fn two_imports_finishing_together_register_under_distinct_ids() {
        let mut document = import_document(Vec::new());
        let intent = LutImportIntent::Apply {
            clip: None,
            stage: ColorStage::Look,
        };

        // Frame 1: the first response is taken and submitted.
        let first = lut_import_operations(&document, sample_import(), &intent).expect("builds");
        let reservation = Some(LutImportReservation {
            session_id: 1,
            asset: first.asset.id,
        });

        // The batch is in flight, so the second response waits rather than
        // being built against a document that does not record the first id.
        assert!(!lut_import_is_ready(reservation, Some(&document)));
        // Building it anyway is exactly the bug: a duplicate id.
        let collided = lut_import_operations(&document, sample_import(), &intent).expect("builds");
        assert_eq!(collided.asset.id, first.asset.id);

        // Frame 2: the first batch has landed.
        kinewright_core::apply_batch(&mut document, &first.operations).expect("first accepted");
        assert!(lut_import_is_ready(reservation, Some(&document)));

        let second = lut_import_operations(&document, sample_import(), &intent).expect("builds");
        assert_ne!(second.asset.id, first.asset.id);
        kinewright_core::apply_batch(&mut document, &second.operations).expect("second accepted");

        assert_eq!(document.lut_assets.len(), 2);
        assert_eq!(
            document
                .lut_assets
                .iter()
                .map(|asset| asset.id)
                .collect::<Vec<_>>(),
            vec![first.asset.id, second.asset.id]
        );
        document.validate().expect("both records are valid");
    }

    /// A reservation whose session is gone retires immediately: there is no
    /// document left for a later batch to collide with, and holding it would
    /// stall every import in every other project.
    #[test]
    fn a_reservation_for_a_closed_session_never_stalls_the_queue() {
        let reservation = Some(LutImportReservation {
            session_id: 7,
            asset: LutAssetId(3),
        });
        assert!(lut_import_is_ready(reservation, None));
        assert!(lut_import_is_ready(None, None));
    }

    /// CC4 §2.2: a Save As that completes while the import worker runs moves
    /// the store root, so the bytes land beside the *old* project file. They
    /// are placed in the root the project now claims before the record that
    /// resolves against it is written.
    #[test]
    fn an_import_that_raced_a_save_as_is_placed_in_the_current_store() {
        let origin = TempDirectory::new("cc4-import-race-origin");
        let destination = TempDirectory::new("cc4-import-race-destination");
        let worker = LutStore::for_project(&origin.path("edit.kinewright")).expect("origin root");
        let current =
            LutStore::for_project(&destination.path("edit.kinewright")).expect("new root");

        let source = origin.path("look.cube");
        std::fs::write(
            &source,
            "LUT_3D_SIZE 2\n             0.0 0.0 0.0\n1.0 0.0 0.0\n0.0 1.0 0.0\n1.0 1.0 0.0\n             0.0 0.0 1.0\n1.0 0.0 1.0\n0.0 1.0 1.0\n1.0 1.0 1.0\n",
        )
        .expect("the fixture .cube writes");
        let import = worker
            .import_lut_asset(&source)
            .expect("the worker imports");
        let asset = import.into_lut_asset(LutAssetId(1));

        // Before the copy the new root resolves nothing.
        assert_eq!(
            current.availability(&asset).kind,
            kinewright_core::LutAvailabilityKind::Missing
        );

        realign_import_store(&worker, &current, &asset).expect("the bytes move");

        assert_eq!(
            current.availability(&asset).kind,
            kinewright_core::LutAvailabilityKind::Verified
        );
        // Idempotent, and a no-op when the root never moved.
        realign_import_store(&worker, &current, &asset).expect("a second call is harmless");
        realign_import_store(&worker, &worker, &asset).expect("the same root copies nothing");
    }

    /// CC4 §2.2: a Save As onto a *refused* root while the import worker runs
    /// leaves the session with no store at all. That is a failure, not an
    /// exemption from realignment: the bytes are still beside the old project
    /// file, so the batch is dropped with the store's own reason instead of
    /// registering an unreachable asset as an imported look.
    #[test]
    fn an_import_that_raced_a_save_as_onto_a_refused_root_is_dropped_with_the_reason() {
        let origin = TempDirectory::new("cc4-import-race-refused");
        let worker = LutStore::for_project(&origin.path("edit.kinewright")).expect("origin root");
        let source = origin.path("look.cube");
        std::fs::write(
            &source,
            "LUT_3D_SIZE 2\n0.0 0.0 0.0\n1.0 0.0 0.0\n0.0 1.0 0.0\n1.0 1.0 0.0\n0.0 0.0 1.0\n1.0 0.0 1.0\n0.0 1.0 1.0\n1.0 1.0 1.0\n",
        )
        .expect("the fixture .cube writes");
        let asset = worker
            .import_lut_asset(&source)
            .expect("the worker imports")
            .into_lut_asset(LutAssetId(1));

        let refusal = "lut_store_root_invalid: the derived store root is not a directory";
        let message = import_realignment_failure("Gate look", &worker, None, Some(refusal), &asset)
            .expect("a session with no store cannot receive the bytes");
        assert!(message.contains("Gate look"), "{message}");
        assert!(message.contains(refusal), "{message}");
        assert!(
            message.contains("was not registered"),
            "the operator must be told the look did not land: {message}"
        );
        assert!(
            !message.contains("project_not_saved"),
            "a refused root is not the save recovery: {message}"
        );

        // With no typed refusal the honest reason is the save recovery.
        let unsaved = import_realignment_failure("Gate look", &worker, None, None, &asset)
            .expect("an unsaved project cannot receive the bytes either");
        assert!(unsaved.contains(PROJECT_NOT_SAVED_MESSAGE), "{unsaved}");

        // A usable store that already owns the bytes is not a failure.
        assert_eq!(
            import_realignment_failure("Gate look", &worker, Some(&worker), None, &asset),
            None
        );
    }

    #[test]
    fn the_project_not_saved_message_names_the_store_and_the_recovery() {
        assert!(PROJECT_NOT_SAVED_MESSAGE.contains("project_not_saved"));
        assert!(PROJECT_NOT_SAVED_MESSAGE.contains("save the project first"));
        assert!(PROJECT_NOT_SAVED_MESSAGE.contains("kinewright-assets"));
    }

    #[test]
    fn a_failure_message_carries_the_file_and_the_typed_code() {
        let error =
            MediaError::Backend("lut_size_out_of_range observed=66 allowed=2..=65".to_owned());
        let message = lut_failure_message(Path::new("/looks/huge.cube"), &error);
        assert!(message.contains("/looks/huge.cube"));
        assert!(message.contains("lut_size_out_of_range"));
        assert!(message.contains("observed=66"));
        assert!(message.contains("allowed=2..=65"));
    }
}
