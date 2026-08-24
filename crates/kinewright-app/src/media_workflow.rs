//! Human-facing media availability, relink, and cache controls.
//!
//! The Core document remains the source of truth for edits.  This module owns
//! the machine-local observations and the small amount of asynchronous UI
//! state needed to turn a probed replacement into exactly one
//! `RelinkAsset` operation.

use std::{collections::HashMap, path::PathBuf, sync::Arc, thread};

use eframe::egui;
use kinewright_core::{
    AssetId, ClipContent, Command, MediaAsset, MediaAvailabilityKind, MediaAvailabilityStatus,
    MediaCacheClearResult, MediaCacheFamily, MediaCacheFamilyStatus, MediaError,
    MediaSourceFingerprint, Operation, RelinkCandidate, TimeCode, TimelineRevision,
};

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

#[derive(Debug)]
pub(crate) struct MediaStatusResponse {
    session_id: u64,
    asset_id: AssetId,
    request_id: u64,
    path: PathBuf,
    fingerprint: MediaSourceFingerprint,
    status: MediaAvailabilityStatus,
}
pub(crate) type CacheClearResponse = (MediaCacheFamily, Result<MediaCacheClearResult, MediaError>);

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

    pub(crate) fn begin(&mut self, session_id: u64, asset: &MediaAsset) -> Option<u64> {
        if self.status(session_id, asset).is_some() {
            return None;
        }
        let key = (session_id, asset.id, asset.path.clone());
        if self.pending.contains_key(&key) {
            return None;
        }
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.pending.insert(key, self.next_request_id);
        Some(self.next_request_id)
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
            },
        );
        true
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

    pub(crate) fn ensure_media_status(
        &mut self,
        asset: &MediaAsset,
    ) -> Option<MediaAvailabilityStatus> {
        let session_id = self.focused().id;
        if let Some(status) = self.media_statuses.status(session_id, asset) {
            return Some(status);
        }
        if let Some(request_id) = self.media_statuses.begin(session_id, asset) {
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
                .is_err()
            {
                self.media_statuses.invalidate(session_id, asset.id);
            }
        }
        None
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
                .is_err()
            {
                self.media_statuses.invalidate(session_id, asset.id);
            }
        }
        None
    }

    pub(crate) fn media_status_for_asset(
        &mut self,
        asset: &MediaAsset,
    ) -> Option<MediaAvailabilityStatus> {
        self.ensure_media_status(asset)
    }

    fn handle_media_status_response(&mut self, response: MediaStatusResponse) {
        if !self.media_statuses.accepts_response(
            response.session_id,
            response.asset_id,
            &response.path,
            response.request_id,
        ) {
            return;
        }
        let Some(project_index) = session_index_by_id(response.session_id, &self.projects) else {
            return;
        };
        let Some(asset) = self.projects[project_index]
            .document
            .asset(response.asset_id)
        else {
            return;
        };
        if asset.path != response.path || asset.source_fingerprint != response.fingerprint {
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
            response.path,
            response.fingerprint,
            response.status,
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
            self.handle_media_status_response(response);
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kinewright_core::{ColorDescription, MediaKind, Rational, TimeCode};

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
}
