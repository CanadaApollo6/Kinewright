mod app;
mod captions;
mod chat_ui;
mod color_qc_ui;
mod color_scopes_ui;
mod color_ui;
mod color_wheel_widget;
mod curve_editor_widget;
mod edit_diff;
mod error_ui;
mod export_ui;
mod grain;
mod icons;
mod incident_ui;
mod inspector_ui;
mod investigator;
mod ja1_attachment;
mod ja1_chat;
#[cfg(test)]
mod ja_harness;
mod keys;
mod light;
mod look_browser_ui;
mod matte_overlay_ui;
mod media_bin;
mod media_workflow;
mod mixer_pane_ui;
mod mixer_ui;
mod motion;
mod motion_probe;
mod performance;
mod performance_gpu;
mod presenter;
mod preview_ui;
mod project;
mod recording;
mod recovery;
mod screenshot;
mod settings_ui;
mod slash;
mod solo_ui;
mod theme;
mod timeline_ui;
mod transcript_edit;
mod transcript_ui;
mod transport;
mod visual_cache;

fn main() -> eframe::Result {
    if motion_probe::spring_report() {
        return Ok(());
    }
    app::run()
}
