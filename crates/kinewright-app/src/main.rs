mod app;
mod captions;
mod chat_ui;
mod color_ui;
mod edit_diff;
mod error_ui;
mod export_ui;
mod icons;
mod inspector_ui;
mod keys;
mod media_bin;
mod preview_ui;
mod project;
mod recording;
mod recovery;
mod screenshot;
mod settings_ui;
mod slash;
mod theme;
mod timeline_ui;
mod transcript_edit;
mod transcript_ui;
mod transport;
mod visual_cache;

fn main() -> eframe::Result {
    app::run()
}
