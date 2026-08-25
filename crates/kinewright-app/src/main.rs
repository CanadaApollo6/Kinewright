mod app;
mod captions;
mod chat_ui;
mod color_scopes_ui;
mod color_ui;
mod color_wheel_widget;
mod curve_editor_widget;
mod edit_diff;
mod error_ui;
mod export_ui;
mod icons;
mod inspector_ui;
mod keys;
mod look_browser_ui;
mod media_bin;
mod media_workflow;
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
