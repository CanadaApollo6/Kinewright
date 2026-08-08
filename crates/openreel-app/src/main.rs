mod app;
mod chat_ui;
mod export_ui;
mod error_ui;
mod keys;
mod media_bin;
mod preview_ui;
mod recovery;
mod timeline_ui;
mod transport;

fn main() -> eframe::Result {
    app::run()
}
