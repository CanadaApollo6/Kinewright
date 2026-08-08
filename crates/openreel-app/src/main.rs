use openreel_core::Document;

struct OpenReelApp {
    _document: Document,
}

impl eframe::App for OpenReelApp {
    fn ui(&mut self, _ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {}
}

fn main() -> eframe::Result {
    eframe::run_native(
        "OpenReel",
        eframe::NativeOptions::default(),
        Box::new(|_creation_context| {
            Ok(Box::new(OpenReelApp {
                _document: Document::default(),
            }))
        }),
    )
}
