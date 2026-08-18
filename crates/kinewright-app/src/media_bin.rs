use std::{sync::Arc, thread};

use eframe::egui;
use kinewright_core::{
    AssetId, ClipId, MediaKind, Operation, ThreePointMode, TimeCode, Track, TrackId, TrackKind,
};

use crate::{
    app::KinewrightApp,
    icons::{self, Icon},
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
            self.projects[project_index].selected_asset = Some(asset_id);
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
        self.projects[project_index].selected_asset = Some(asset_id);
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
        let media = Arc::clone(&self.analysis);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for asset in assets {
                    let selected = self.focused_mut().selected_asset == Some(asset.id);
                    let response = theme::card_frame(selected).show(ui, |ui| {
                        let width = ui.available_width().max(120.0);
                        let height = (width * 9.0 / 16.0).clamp(72.0, 126.0);
                        let (thumbnail_rect, thumbnail_response) =
                            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
                        ui.painter()
                            .rect_filled(thumbnail_rect, radius::SM, color::MEDIA_SHADOW);
                        if ui.clip_rect().intersects(thumbnail_rect)
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
        let duration = self
            .focused()
            .document
            .asset(asset_id)
            .map_or(TimeCode::ZERO, |asset| asset.duration);
        let session = self.focused_mut();
        if session.selected_asset != Some(asset_id) {
            session.source_in = TimeCode::ZERO;
            session.source_out = duration;
        }
        session.selected_asset = Some(asset_id);
    }

    fn source_monitor_controls(&mut self, ui: &mut egui::Ui) {
        let Some(asset_id) = self.focused().selected_asset else {
            return;
        };
        let Some(asset) = self.focused().document.asset(asset_id).cloned() else {
            return;
        };
        if self.focused().source_out <= self.focused().source_in
            || self.focused().source_out > asset.duration
        {
            let session = self.focused_mut();
            session.source_in = TimeCode::ZERO;
            session.source_out = asset.duration;
        }

        theme::card_frame(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("SOURCE").font(theme::semibold(type_size::CAPTION)));
                ui.colored_label(color::TEXT_MUTED, &asset.name);
            });
            let mut source_in = self.focused().source_in.0;
            let mut source_out = self.focused().source_out.0;
            ui.horizontal(|ui| {
                ui.label("In");
                ui.add(
                    egui::DragValue::new(&mut source_in)
                        .range(0..=asset.duration.0.saturating_sub(1)),
                );
                ui.label("Out");
                ui.add(egui::DragValue::new(&mut source_out).range(1..=asset.duration.0));
            });
            source_in = source_in.clamp(0, asset.duration.0.saturating_sub(1));
            source_out = source_out.clamp(source_in.saturating_add(1), asset.duration.0);
            self.focused_mut().source_in = TimeCode(source_in);
            self.focused_mut().source_out = TimeCode(source_out);

            ui.horizontal(|ui| {
                for (label, mode) in [
                    ("Insert at playhead", ThreePointMode::Insert),
                    ("Overwrite", ThreePointMode::Overwrite),
                ] {
                    if ui.button(label).clicked() {
                        let track = self
                            .focused()
                            .document
                            .tracks
                            .iter()
                            .find(|track| asset.kind.supports(track.kind))
                            .map(|track| track.id);
                        if let Some(track) = track {
                            self.send_operation(Operation::ThreePointEdit {
                                track,
                                asset: asset.id,
                                source_in: Some(TimeCode(source_in)),
                                source_out: Some(TimeCode(source_out)),
                                timeline_in: Some(self.focused().position),
                                timeline_out: None,
                                mode,
                            });
                        } else {
                            self.record_error(
                                "Source monitor",
                                format!("No compatible track exists for {}", asset.name),
                            );
                        }
                    }
                }
            });
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
        };
        let document = Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
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
