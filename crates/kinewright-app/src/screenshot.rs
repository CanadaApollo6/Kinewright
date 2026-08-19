use std::{
    fs,
    path::PathBuf,
    process,
    sync::Arc,
    time::{Duration, Instant},
};

use eframe::egui;

const SCREENSHOT_ENV: &str = "KINEWRIGHT_SCREENSHOT_TO";
const SCREENSHOT_DELAY_ENV: &str = "KINEWRIGHT_SCREENSHOT_AFTER_MS";

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
            match save_screenshot(&output, &image) {
                Ok(()) => process::exit(0),
                Err(error) => {
                    eprintln!("screenshot failed: {error}");
                    process::exit(1);
                }
            }
        }

        if !self.requested && ctx.cumulative_pass_nr() >= 2 && Instant::now() >= self.ready_at {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.requested = true;
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

fn save_screenshot(output: &PathBuf, image: &egui::ColorImage) -> Result<(), String> {
    let width = u32::try_from(image.size[0]).map_err(|error| error.to_string())?;
    let height = u32::try_from(image.size[1]).map_err(|error| error.to_string())?;
    if width == 0 || height == 0 {
        return Err("screenshot capture was empty".to_owned());
    }
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let rgba = image
        .pixels
        .iter()
        .flat_map(eframe::egui::Color32::to_srgba_unmultiplied)
        .collect::<Vec<_>>();
    image::save_buffer(output, &rgba, width, height, image::ColorType::Rgba8)
        .map_err(|error| error.to_string())
}
