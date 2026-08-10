use std::time::{Duration, Instant};

use eframe::egui;

use crate::app::OpenReelApp;

pub(crate) struct ErrorEntry {
    elapsed: Duration,
    source: &'static str,
    message: String,
}

pub(crate) struct ErrorLog {
    started: Instant,
    entries: Vec<ErrorEntry>,
}

impl Default for ErrorLog {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            entries: Vec::new(),
        }
    }
}

impl ErrorLog {
    pub(crate) fn push(&mut self, source: &'static str, message: impl Into<String>) {
        self.entries.push(ErrorEntry {
            elapsed: self.started.elapsed(),
            source,
            message: message.into(),
        });
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

impl OpenReelApp {
    pub(crate) fn record_error(&mut self, source: &'static str, message: impl Into<String>) {
        let message = message.into();
        self.status.clone_from(&message);
        self.error_log.push(source, message);
        self.error_log_open = true;
    }

    pub(crate) fn show_error_log(&mut self, ctx: &egui::Context) {
        if !self.error_log_open {
            return;
        }
        let mut clear = false;
        egui::Window::new("Error log")
            .open(&mut self.error_log_open)
            .default_width(620.0)
            .default_height(240.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("{} error(s)", self.error_log.entries.len()));
                    if ui.button("Clear").clicked() {
                        clear = true;
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for entry in &self.error_log.entries {
                            let total_seconds = entry.elapsed.as_secs();
                            let minutes = total_seconds / 60;
                            let seconds = total_seconds % 60;
                            ui.horizontal_wrapped(|ui| {
                                ui.monospace(format!(
                                    "+{minutes:02}:{seconds:02}.{:03}",
                                    entry.elapsed.subsec_millis()
                                ));
                                ui.strong(entry.source);
                                ui.label(&entry.message);
                            });
                        }
                    });
            });
        if clear {
            self.error_log.clear();
        }
    }
}
