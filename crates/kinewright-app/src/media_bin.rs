use std::{sync::Arc, thread};

use eframe::egui;
use kinewright_core::{AssetId, ClipId, MediaKind, Operation, TimeCode, Track, TrackId, TrackKind};

use crate::{
    app::KinewrightApp,
    color_ui::{
        ASSUME_SDR_REC709_TOOLTIP, SourceColorDisplay, assume_sdr_rec709_operation,
        source_color_display,
    },
    icons::{self, Icon},
    media_workflow::{paint_source_status, source_display_state},
    theme::{self, color, radius, size, space, type_size},
    timeline_ui::format_timecode,
};

impl KinewrightApp {
    pub(crate) fn choose_media(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mov", "mkv", "webm", "avi"])
            .pick_file()
        else {
            return;
        };
        self.import_media_path(path);
    }

    /// Probe a file into the focused project; on success it flows down the
    /// one-gesture import pipeline (asset, timeline, monitor cue). Shared by
    /// the file dialog, drag-and-drop, and /import.
    pub(crate) fn import_media_path(&mut self, path: std::path::PathBuf) {
        self.status = format!("Probing {}…", path.display());
        let session_id = self.focused().id;
        let media = Arc::clone(&self.analysis);
        let result_tx = self.probe_tx.clone();
        thread::Builder::new()
            .name("kinewright-probe".to_owned())
            .spawn(move || {
                let result = media.probe(&path);
                let _ = result_tx.send((session_id, path, result));
            })
            .expect("failed to spawn media probe worker");
    }

    /// Files dropped anywhere on the window import into the focused project -
    /// the media column is optional, not a prerequisite for getting footage in.
    pub(crate) fn import_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<std::path::PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        for path in dropped {
            self.import_media_path(path);
        }
    }

    pub(crate) fn add_asset_to_timeline(&mut self, asset_id: AssetId) {
        self.add_asset_to_timeline_for(self.focused_project, asset_id);
    }

    pub(crate) fn add_asset_to_timeline_for(&mut self, project_index: usize, asset_id: AssetId) {
        let Some(project) = self.projects.get(project_index) else {
            return;
        };
        let Some(asset) = project.document.asset(asset_id).cloned() else {
            self.record_error("Operations", format!("Asset {asset_id} no longer exists"));
            return;
        };
        let clip_start = project.document.duration;
        if asset.kind == MediaKind::AudioVideo {
            if self.add_audio_video_asset_to_timeline(project_index, &asset) {
                self.projects[project_index].position = clip_start;
            }
            self.projects[project_index].cue_source_asset(asset_id);
            return;
        }
        let Some(track_id) = self.projects[project_index]
            .document
            .tracks
            .iter()
            .find(|track| asset.kind.supports(track.kind))
            .map(|track| track.id)
        else {
            self.record_error(
                "Operations",
                format!("No compatible track exists for {}", asset.name),
            );
            return;
        };
        let operation = Operation::AddClip {
            track: track_id,
            asset: asset.id,
            at: clip_start,
            source: TimeCode::ZERO..asset.duration,
        };
        if self.projects[project_index]
            .core
            .send(kinewright_core::Command::Do(operation))
            .is_err()
        {
            self.record_error("Operations", "Core actor stopped while adding media");
        }
        self.projects[project_index].position = clip_start;
        self.projects[project_index].cue_source_asset(asset_id);
    }

    fn add_audio_video_asset_to_timeline(
        &mut self,
        project_index: usize,
        asset: &kinewright_core::MediaAsset,
    ) -> bool {
        match audio_video_placement_operations(&self.projects[project_index].document, asset) {
            Ok(operations) => {
                if self.projects[project_index]
                    .core
                    .send(kinewright_core::Command::DoBatch(operations))
                    .is_err()
                {
                    self.record_error("Operations", "Core actor stopped while adding A/V media");
                    false
                } else {
                    true
                }
            }
            Err(error) => {
                self.record_error("Operations", error);
                false
            }
        }
    }

    // The media-bin immediate-mode pass keeps filtering, selection, and actions together.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn media_bin(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Media");
            let catalog = &self.focused().document.catalog;
            if !catalog.is_empty() {
                ui.colored_label(
                    color::TEXT_MUTED,
                    format!(
                        "{} bins · {} string-outs · {} sync groups",
                        catalog.bins.len(),
                        catalog.string_outs.len(),
                        catalog.sync_groups.len()
                    ),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("Refresh")
                    .on_hover_text("Recheck media files and source fingerprints")
                    .clicked()
                {
                    self.refresh_media_statuses_for_focused_project();
                }
                if ui
                    .small_button("Cache")
                    .on_hover_text("Inspect preview and derived-media caches")
                    .clicked()
                {
                    self.open_media_cache_dialog();
                }
                let button =
                    egui::Button::image_and_text(Icon::Import.image(size::ICON_MD), "Import")
                        .image_tint_follows_text_color(true)
                        .corner_radius(radius::SM);
                if ui.add(button).on_hover_text("Import media…").clicked() {
                    self.choose_media();
                }
            });
        });
        ui.add_space(space::ONE);
        ui.separator();
        ui.add_space(space::ONE);
        self.source_monitor_controls(ui);
        if self.focused().document.media_pool.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(space::EIGHT);
                ui.add(Icon::Filmstrip.image(32.0).tint(color::TEXT_MUTED));
                ui.add_space(space::TWO);
                ui.label("No media imported");
                ui.colored_label(color::TEXT_MUTED, "Import a clip to begin editing.");
            });
            return;
        }
        let assets = self.focused().document.media_pool.clone();
        let assets = assets
            .into_iter()
            .map(|asset| {
                let status = self.media_status_for_asset(&asset);
                (asset, status)
            })
            .collect::<Vec<_>>();
        let media = Arc::clone(&self.analysis);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (asset, status) in assets {
                    let source_state = source_display_state(status.as_ref());
                    let selected = self.focused_mut().selected_asset == Some(asset.id);
                    let response = theme::card_frame(selected).show(ui, |ui| {
                        let width = ui.available_width().max(120.0);
                        let height = (width * 9.0 / 16.0).clamp(72.0, 126.0);
                        let (thumbnail_rect, thumbnail_response) =
                            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
                        ui.painter()
                            .rect_filled(thumbnail_rect, radius::SM, color::MEDIA_SHADOW);
                        if ui.clip_rect().intersects(thumbnail_rect)
                            && !source_state.blocks_preview()
                            && matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo)
                        {
                            if let Some(texture) = self.visual_cache.thumbnail(
                                media.as_ref(),
                                &asset,
                                TimeCode::ZERO,
                                256,
                            ) {
                                ui.painter().image(
                                    texture.id(),
                                    thumbnail_rect,
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    color::MEDIA_TINT_78,
                                );
                            }
                        } else {
                            Icon::Waveform.image(28.0).tint(color::TEXT_MUTED).paint_at(
                                ui,
                                egui::Rect::from_center_size(
                                    thumbnail_rect.center(),
                                    egui::vec2(28.0, 28.0),
                                ),
                            );
                        }
                        if source_state.blocks_preview() {
                            ui.painter().rect_filled(
                                thumbnail_rect,
                                radius::SM,
                                egui::Color32::from_black_alpha(150),
                            );
                            ui.painter().text(
                                thumbnail_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                source_state.label(),
                                theme::semibold(type_size::CAPTION),
                                color::STATUS_DANGER,
                            );
                        }
                        let duration = format_timecode(asset.duration, asset.fps);
                        let duration_size = egui::vec2(76.0, 18.0);
                        let badge = egui::Rect::from_min_size(
                            thumbnail_rect.right_bottom()
                                - duration_size
                                - egui::vec2(space::ONE, space::ONE),
                            duration_size,
                        );
                        ui.painter()
                            .rect_filled(badge, radius::XS, color::MEDIA_SCRIM_78);
                        ui.painter().text(
                            badge.center(),
                            egui::Align2::CENTER_CENTER,
                            duration,
                            egui::FontId::new(type_size::CAPTION, egui::FontFamily::Monospace),
                            color::TEXT_PRIMARY,
                        );
                        ui.add_space(space::ONE);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(&asset.name)
                                        .font(theme::semibold(type_size::BODY)),
                                );
                                ui.colored_label(color::TEXT_MUTED, asset_metadata(&asset));
                                paint_source_status(ui, source_state);
                                if source_state.blocks_preview() {
                                    ui.colored_label(
                                        color::STATUS_DANGER,
                                        source_state.description(),
                                    );
                                }
                                if let Some(display) = source_color_display(&asset) {
                                    source_color_label(ui, display);
                                }
                                if let Some(bin) = self
                                    .focused()
                                    .document
                                    .catalog
                                    .bins
                                    .iter()
                                    .find(|bin| bin.assets.contains(&asset.id))
                                {
                                    ui.colored_label(
                                        color::TEXT_MUTED,
                                        format!("BIN · {}", bin.name),
                                    );
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icons::button(ui, Icon::Add, "Add to timeline").clicked() {
                                        self.add_asset_to_timeline(asset.id);
                                    }
                                },
                            );
                        });
                        thumbnail_response
                    });
                    theme::paint_raised_lighting(
                        ui.painter(),
                        response.response.rect,
                        radius::px(radius::MD),
                    );
                    let card_response = ui.interact(
                        response.response.rect,
                        ui.make_persistent_id(("media-card", asset.id.0)),
                        egui::Sense::click(),
                    );
                    if card_response.clicked() || response.inner.clicked() {
                        self.select_source_asset(asset.id);
                    }
                    ui.add_space(space::ONE);
                }
            });
    }

    fn select_source_asset(&mut self, asset_id: AssetId) {
        self.focused_mut().cue_source_asset(asset_id);
    }

    fn source_monitor_controls(&mut self, ui: &mut egui::Ui) {
        let Some(asset_id) = self.focused().selected_asset else {
            return;
        };
        let Some(asset) = self.focused().document.asset(asset_id).cloned() else {
            return;
        };
        theme::card_frame(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("SOURCE").font(theme::semibold(type_size::CAPTION)));
                ui.colored_label(color::TEXT_MUTED, &asset.name);
            });
            let status = self.media_status_for_asset(&asset);
            let source_state = source_display_state(status.as_ref());
            ui.horizontal(|ui| {
                paint_source_status(ui, source_state);
                if ui
                    .button("Relink…")
                    .on_hover_text("Choose a replacement and verify its source fingerprint")
                    .clicked()
                {
                    self.choose_relink_for_asset(asset.id);
                }
            });
            ui.colored_label(
                if source_state.blocks_preview() {
                    color::STATUS_DANGER
                } else {
                    color::TEXT_MUTED
                },
                source_state.description(),
            );
            if let Some(display) = source_color_display(&asset) {
                source_color_label(ui, display);
                if ui
                    .button("Assume SDR Rec.709 metadata")
                    .on_hover_text(ASSUME_SDR_REC709_TOOLTIP)
                    .clicked()
                {
                    self.send_operation(assume_sdr_rec709_operation(&asset));
                }
            }
            let session = self.focused();
            let video_route = session
                .source_video_target
                .map_or_else(|| "off".to_owned(), |track| format!("V → {track}"));
            let audio_route = session
                .source_audio_target
                .map_or_else(|| "off".to_owned(), |track| format!("A → {track}"));
            ui.colored_label(
                color::TEXT_MUTED,
                format!(
                    "Source viewer · frame {} · In {} · Out {} · {video_route} · {audio_route}",
                    session.source_position, session.source_in, session.source_out
                ),
            );
            ui.colored_label(
                color::TEXT_MUTED,
                "Insert and overwrite are available in the Source viewer.",
            );
        });
        ui.add_space(space::ONE);
    }
}

fn audio_video_placement_operations(
    document: &kinewright_core::Document,
    asset: &kinewright_core::MediaAsset,
) -> Result<Vec<Operation>, String> {
    let video_track = document
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .map(|track| track.id)
        .ok_or_else(|| format!("No video track exists for {}", asset.name))?;
    let mut operations = Vec::new();
    let audio_track = if let Some(track) = document
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Audio)
    {
        track.id
    } else {
        let track_id = next_track_id(document).ok_or("Track id space is exhausted")?;
        operations.push(Operation::AddTrack {
            track: Track {
                id: track_id,
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            },
        });
        track_id
    };
    let first_clip = next_clip_id(document).ok_or("Clip id space is exhausted")?;
    let second_clip = first_clip
        .0
        .checked_add(1)
        .map(ClipId)
        .ok_or("Clip id space is exhausted")?;
    let at = document.duration;
    let source = TimeCode::ZERO..asset.duration;
    operations.extend([
        Operation::AddClip {
            track: video_track,
            asset: asset.id,
            at,
            source: source.clone(),
        },
        Operation::AddClip {
            track: audio_track,
            asset: asset.id,
            at,
            source,
        },
        Operation::LinkClips {
            clips: vec![first_clip, second_clip],
        },
    ]);
    Ok(operations)
}

fn next_track_id(document: &kinewright_core::Document) -> Option<TrackId> {
    document
        .tracks
        .iter()
        .map(|track| track.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(TrackId)
}

fn next_clip_id(document: &kinewright_core::Document) -> Option<ClipId> {
    document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .map(|clip| clip.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(ClipId)
}

fn asset_metadata(asset: &kinewright_core::MediaAsset) -> String {
    let kind = match asset.kind {
        MediaKind::Video => "VIDEO",
        MediaKind::Audio => "AUDIO",
        MediaKind::AudioVideo => "A/V",
    };
    let resolution = asset
        .resolution
        .map(|(width, height)| format!(" · {width}×{height}"))
        .unwrap_or_default();
    format!(
        "{kind}{resolution} · {}/{} fps",
        asset.fps.numerator(),
        asset.fps.denominator()
    )
}

fn source_color_label(ui: &mut egui::Ui, display: SourceColorDisplay) {
    let text_color = if display.blocking {
        color::STATUS_DANGER
    } else if display.warning {
        color::STATUS_WARNING
    } else {
        color::TEXT_MUTED
    };
    ui.add(egui::Label::new(egui::RichText::new(display.summary).color(text_color)).wrap());
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kinewright_core::{Document, MediaAsset, Rational};

    use super::*;

    #[test]
    fn audio_video_placement_creates_audio_track_and_links_both_clips() {
        let fps = Rational::new(30, 1).unwrap();
        let asset = MediaAsset {
            id: AssetId(4),
            path: PathBuf::from("interview.mp4"),
            name: "interview.mp4".to_owned(),
            duration: TimeCode(90),
            fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        let document = Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            }],
            media_pool: vec![asset.clone()],
            markers: Vec::new(),
            fps,
            resolution: (1_920, 1_080),
            duration: TimeCode::ZERO,
        };
        assert_eq!(
            audio_video_placement_operations(&document, &asset).unwrap(),
            vec![
                Operation::AddTrack {
                    track: Track {
                        id: TrackId(2),
                        kind: TrackKind::Audio,
                        sync_lock: true,
                        clips: Vec::new(),
                    },
                },
                Operation::AddClip {
                    track: TrackId(1),
                    asset: AssetId(4),
                    at: TimeCode::ZERO,
                    source: TimeCode::ZERO..TimeCode(90),
                },
                Operation::AddClip {
                    track: TrackId(2),
                    asset: AssetId(4),
                    at: TimeCode::ZERO,
                    source: TimeCode::ZERO..TimeCode(90),
                },
                Operation::LinkClips {
                    clips: vec![ClipId(1), ClipId(2)],
                },
            ]
        );
    }
}
