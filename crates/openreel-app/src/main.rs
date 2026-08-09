mod app;
mod chat_ui;
mod error_ui;
mod export_ui;
mod icons;
mod keys;
mod media_bin;
mod preview_ui;
mod recovery;
mod screenshot;
mod theme;
mod timeline_ui;
mod transcript_ui;
mod transport;
mod visual_cache;

fn main() -> eframe::Result {
    app::run()
}
