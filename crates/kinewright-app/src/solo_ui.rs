//! MO2 R26: the person's `preview_solo` — the header's Solo buttons run the
//! agent capability itself on a worker and show its strip and report.

use std::sync::Arc;

use crossbeam_channel::Receiver;
use eframe::egui;
use kinewright_agent::{SoloArgs, SoloError, SoloStrip};
use kinewright_core::ClipId;

use crate::{app::KinewrightApp, theme::color};

type SoloOutcome = Result<(SoloStrip, egui::TextureHandle), SoloError>;

/// One solo render: pending on the worker, then its strip or typed refusal.
pub(crate) struct SoloDialog {
    pub(crate) clip: ClipId,
    pub(crate) full_res: bool,
    pending: Receiver<Result<SoloStrip, SoloError>>,
    outcome: Option<SoloOutcome>,
}

impl KinewrightApp {
    /// Render `clip` solo against the focused session's current snapshot.
    pub(crate) fn open_solo_dialog(&mut self, clip: ClipId, full_res: bool) {
        let analysis = Arc::clone(&self.analysis);
        let document = Arc::clone(&self.focused().document);
        let revision = self.focused().revision;
        let (sender, pending) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let args = SoloArgs {
                expected_revision: revision,
                clip_id: clip,
                samples: 8,
                context: None,
                full_res,
            };
            let strip = kinewright_agent::preview_solo(&*analysis, revision, &document, &args);
            let _ = sender.send(strip);
        });
        self.solo_dialog = Some(SoloDialog {
            clip,
            full_res,
            pending,
            outcome: None,
        });
    }

    pub(crate) fn show_solo_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.solo_dialog.as_mut() else {
            return;
        };
        if dialog.outcome.is_none()
            && let Ok(result) = dialog.pending.try_recv()
        {
            let max_side = ctx.input(|input| input.max_texture_side);
            dialog.outcome = Some(result.map(|strip| {
                let image = strip_image(&strip.image, max_side);
                let texture = ctx.load_texture("solo-strip", image, egui::TextureOptions::LINEAR);
                (strip, texture)
            }));
        }
        let mut open = true;
        let mode = if dialog.full_res { " (full-res)" } else { "" };
        egui::Window::new(format!("Solo clip {}{mode}", dialog.clip))
            .open(&mut open)
            .resizable(true)
            .show(ctx, |ui| match &dialog.outcome {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Rendering the solo strip…");
                    });
                    ctx.request_repaint();
                }
                Some(Ok((strip, texture))) => {
                    let report = &strip.report;
                    let provenance = &report["provenance"];
                    ui.label(format!(
                        "{} · {} of {} samples · {} ({})",
                        report["context"].as_str().unwrap_or_default(),
                        report["emitted"],
                        report["requested"],
                        provenance["adapter"].as_str().unwrap_or_default(),
                        provenance["backend"].as_str().unwrap_or_default(),
                    ));
                    if report["pairs"] == true {
                        ui.colored_label(color::TEXT_MUTED, "Top: BEFORE · bottom: AFTER");
                    }
                    if let Some(degraded) = report["degraded"].as_str() {
                        ui.colored_label(color::STATUS_WARNING, format!("degraded: {degraded}"));
                    }
                    let inactive: Vec<_> = (report["samples"].as_array().into_iter().flatten())
                        .filter(|sample| sample["active"] == false)
                        .map(|sample| sample["frame"].to_string())
                        .collect();
                    if !inactive.is_empty() {
                        let frames = inactive.join(", ");
                        ui.colored_label(color::TEXT_MUTED, format!("disabled at {frames}"));
                    }
                    let width = ui.available_width().min(texture.size_vec2().x);
                    ui.add(egui::Image::new(texture).max_width(width));
                }
                Some(Err(error)) => {
                    ui.colored_label(color::STATUS_DANGER, format!("{}: {error}", error.code()));
                }
            });
        if !open {
            self.solo_dialog = None;
        }
    }
}

/// The strip as an egui image, downscaled to fit the GPU's texture side (a
/// full-res or many-sample strip can outgrow it).
fn strip_image(strip: &kinewright_core::RgbaImage, max_side: usize) -> egui::ColorImage {
    let (width, height) = (strip.width as usize, strip.height as usize);
    let longest = width.max(height).max(1);
    let pixels = image::RgbaImage::from_raw(strip.width, strip.height, strip.pixels.clone());
    match pixels {
        Some(pixels) if longest > max_side => {
            let fit = |extent: usize| u32::try_from((extent * max_side / longest).max(1));
            let (Ok(fit_width), Ok(fit_height)) = (fit(width), fit(height)) else {
                return egui::ColorImage::example();
            };
            let small = image::imageops::thumbnail(&pixels, fit_width, fit_height);
            let size = [fit_width as usize, fit_height as usize];
            egui::ColorImage::from_rgba_unmultiplied(size, small.as_raw())
        }
        _ => egui::ColorImage::from_rgba_unmultiplied([width, height], &strip.pixels),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::{Duration, Instant};

    use eframe::egui;
    use kinewright_core::Analysis;
    use kinewright_media::{
        in1_sources::{In1Source, in1_source},
        test_support::single_clip_document,
    };

    use crate::app::KinewrightApp;

    /// Frame the solo dialog until its worker answers (or panic on a hang),
    /// then return one settled frame's painted text.
    pub(crate) fn settle_solo_dialog(app: &mut KinewrightApp) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut time = 0.0;
        loop {
            time += 0.02;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 900.0),
                    )),
                    time: Some(time),
                    ..Default::default()
                },
                |ui| app.show_solo_dialog(ui.ctx()),
            );
            let answered =
                (app.solo_dialog.as_ref()).is_some_and(|dialog| dialog.outcome.is_some());
            if answered && time > 0.1 {
                return crate::theme::painted_text(&output);
            }
            assert!(
                Instant::now() < deadline,
                "the solo worker answered nothing"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// R26: the dialog renders a tagged media clip solo through the agent's
    /// `preview_solo` — strip texture, report line, no refusal.
    #[test]
    fn mo2_the_solo_dialog_shows_the_strip_and_its_report() {
        let engine = kinewright_media::FfmpegMediaEngine::new().expect("the engine starts");
        let fixture = in1_source(In1Source::TaggedMp4);
        let asset = engine.probe(fixture.path()).expect("the fixture probes");
        let (mut app, _engine) = crate::app::in1_tests::in1_harness(single_clip_document(asset));
        let clip = app.focused().document.tracks[0].clips[0].id;
        app.open_solo_dialog(clip, false);
        let painted = settle_solo_dialog(&mut app);
        let dialog = app.solo_dialog.as_ref().expect("still open");
        let Some(Ok((strip, texture))) = &dialog.outcome else {
            panic!("a strip, not a refusal: {painted:?}");
        };
        assert_eq!(strip.report["context"], "isolated");
        assert_eq!(strip.report["pairs"], false);
        let [width, height] = texture.size();
        let side = (strip.image.width.max(strip.image.height)) as usize;
        assert!(
            side > 2_048,
            "the default strip outgrows egui's default texture side"
        );
        assert_eq!(
            width.max(height),
            2_048,
            "downscaled to fit, not refused or panicked"
        );
        assert_eq!(width, strip.image.width as usize * 2_048 / side);
        let emitted = &strip.report["emitted"];
        assert!(
            painted
                .iter()
                .any(|text| text.starts_with(&format!("isolated · {emitted} of 8 samples"))),
            "the report line is shown: {painted:?}"
        );
        crate::app::in1_tests::in1_shutdown(&mut app);
    }
}
