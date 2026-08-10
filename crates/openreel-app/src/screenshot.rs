use std::{
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use eframe::egui;

const SCREENSHOT_ENV: &str = "OPENREEL_SCREENSHOT_TO";
const SCREENSHOT_DELAY_ENV: &str = "OPENREEL_SCREENSHOT_AFTER_MS";

pub(crate) struct ScreenshotCapture {
    output: Option<PathBuf>,
    requested: bool,
    /// Waiting until async visuals (thumbnails, waveforms) have a chance to
    /// arrive makes captures representative, not just fast.
    ready_at: Instant,
}

impl ScreenshotCapture {
    pub(crate) fn from_environment() -> Self {
        let delay_ms = std::env::var(SCREENSHOT_DELAY_ENV)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        Self {
            output: std::env::var_os(SCREENSHOT_ENV).map(PathBuf::from),
            requested: false,
            ready_at: Instant::now() + Duration::from_millis(delay_ms),
        }
    }

    pub(crate) fn update(&mut self, ctx: &egui::Context) {
        let Some(output) = self.output.as_ref() else {
            return;
        };
        let screenshot = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(Arc::clone(image)),
                _ => None,
            })
        });
        if let Some(image) = screenshot {
            let output = output.clone();
            self.output = None;
            let _ = thread::Builder::new()
                .name("openreel-screenshot".to_owned())
                .spawn(move || save_screenshot(output, image));
            return;
        }

        if !self.requested && ctx.cumulative_pass_nr() >= 2 && Instant::now() >= self.ready_at {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.requested = true;
        } else if !self.requested {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

fn save_screenshot(output: PathBuf, image: Arc<egui::ColorImage>) {
    let width = u32::try_from(image.size[0]).unwrap_or_default();
    let height = u32::try_from(image.size[1]).unwrap_or_default();
    if width == 0 || height == 0 {
        return;
    }
    let rgba = image
        .pixels
        .iter()
        .flat_map(eframe::egui::Color32::to_srgba_unmultiplied)
        .collect::<Vec<_>>();
    let _ = image::save_buffer(output, &rgba, width, height, image::ColorType::Rgba8);
}
