use std::{sync::Arc, thread};

use eframe::egui;
use openreel_core::{AssetId, MediaEngine, Operation, TimeCode, TrackKind};

use crate::app::OpenReelApp;

impl OpenReelApp {
    pub(crate) fn choose_media(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mov", "mkv", "webm", "avi"])
            .pick_file()
        else {
            return;
        };
        self.status = format!("Probing {}…", path.display());
        let media = Arc::clone(&self.media);
        let result_tx = self.probe_tx.clone();
        thread::Builder::new()
            .name("openreel-probe".to_owned())
            .spawn(move || {
                let result = media.probe(&path);
                let _ = result_tx.send((path, result));
            })
            .expect("failed to spawn media probe worker");
    }

    pub(crate) fn add_asset_to_timeline(&mut self, asset_id: AssetId) {
        let Some(track) = self
            .document
            .tracks
            .iter()
            .find(|track| track.kind == TrackKind::Video)
        else {
            self.record_error("Operations", "No video track exists");
            return;
        };
        let Some(asset) = self.document.asset(asset_id) else {
            self.record_error("Operations", format!("Asset {asset_id} no longer exists"));
            return;
        };
        self.send_operation(Operation::AddClip {
            track: track.id,
            asset: asset.id,
            at: self.document.duration,
            source: TimeCode::ZERO..asset.duration,
        });
    }

    pub(crate) fn media_bin(&mut self, ui: &mut egui::Ui) {
        ui.heading("Media bin");
        if ui.button("Import media…").clicked() {
            self.choose_media();
        }
        ui.separator();
        if self.document.media_pool.is_empty() {
            ui.label("No imported assets");
            return;
        }
        let assets = self.document.media_pool.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for asset in assets {
                ui.group(|ui| {
                    ui.strong(&asset.name);
                    ui.small(format!(
                        "{} frames · {}/{} fps",
                        asset.duration.0,
                        asset.fps.numerator(),
                        asset.fps.denominator()
                    ));
                    if ui.button("Add to timeline").clicked() {
                        self.add_asset_to_timeline(asset.id);
                    }
                });
            }
        });
    }
}
