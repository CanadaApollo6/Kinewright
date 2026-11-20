mod app;
mod captions;
mod chat_ui;
mod color_qc_ui;
mod color_scopes_ui;
mod color_ui;
mod color_wheel_widget;
mod curve_editor_widget;
mod director_readiness;
mod display_text;
mod edit_diff;
mod error_ui;
mod export_ui;
mod icons;
mod incident_ui;
mod inspector_ui;
mod interchange_ui;
mod investigator;
#[cfg(test)]
mod ix1_tests;
mod ja1_attachment;
mod ja1_chat;
#[cfg(test)]
mod ja_harness;
mod joined_thread;
mod keys;
mod look_browser_ui;
mod manual_edits;
mod matte_overlay_ui;
mod media_access;
mod media_bin;
mod media_workflow;
mod mixer_pane_ui;
mod mixer_ui;
mod performance;
mod presenter;
mod preview_ui;
mod project;
mod recording;
mod recovery;
mod retirement;
mod screenshot;
mod settings_ui;
mod slash;
mod solo_ui;
mod studio;
mod studio_feedback;
mod studio_review;
mod studio_run_view;
mod studio_voice;
mod theme;
mod timeline_ui;
mod transcript_edit;
mod transcript_ui;
mod transport;
mod visual_cache;

fn main() -> eframe::Result {
    #[cfg(target_os = "linux")]
    {
        let mut args = std::env::args_os().skip(1);
        if args.next().as_deref() == Some(std::ffi::OsStr::new("--check-bundle-admission")) {
            let (Some(media), Some(session_parent), None) = (args.next(), args.next(), args.next())
            else {
                eprintln!("usage: Kinewright --check-bundle-admission MEDIA SESSION_DIR");
                std::process::exit(2);
            };
            if let Err(error) = check_bundle_admission(media.as_ref(), session_parent.as_ref()) {
                eprintln!("{error}");
                std::process::exit(1);
            }
            return Ok(());
        }
    }
    // MS2: the confined probe helper and scope supervisor ship beside this
    // executable. Without them, media admission fails as unavailable.
    if let Err(error) =
        kinewright_project::media_sandbox::configure_media_sandbox_beside_executable()
    {
        eprintln!("media sandbox unavailable: {error}");
    }
    let result = app::run();
    if let Err(error) = &result {
        rfd::MessageDialog::new()
            .set_title("Kinewright could not start")
            .set_description(error.to_string())
            .set_level(rfd::MessageLevel::Error)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
    result
}

#[cfg(target_os = "linux")]
fn check_bundle_admission(
    media: &std::path::Path,
    session_parent: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::process::CommandExt;

    use kinewright_project::{
        bundle_records::{BatchId, BranchId},
        editing_session::Ja1EditingSession,
        media_sandbox,
        operation_actor::Core,
        operation_history::{CreationOrigin, HistoryPool, PoolLimits, Role},
        storage_budget::StorageLimits,
    };

    let session_parent = session_parent.canonicalize()?;
    // The sandbox takes its temporary mount root from TMPDIR. Set it for
    // this process on exec, before the probe runtime starts any threads.
    if std::env::temp_dir() != session_parent {
        return Err(std::process::Command::new(std::env::current_exe()?)
            .arg("--check-bundle-admission")
            .arg(media)
            .arg(&session_parent)
            .env("TMPDIR", &session_parent)
            .exec()
            .into());
    }
    media_sandbox::configure_media_sandbox_beside_executable()?;
    let history = HistoryPool::new(PoolLimits::default());
    let core = Core::spawn(
        &history,
        Role::Ordinary,
        CreationOrigin {
            branch: BranchId::fresh()?,
            batch: BatchId::fresh()?,
        },
        kinewright_core::Document::default(),
    )?;
    // The diagnostic uses a headless edit store under session_parent. It
    // never creates an App project session or starts a recovery journal.
    let mut session = Ja1EditingSession::new(&session_parent, &core, StorageLimits::default())?;
    let (asset, _) = session.import_owned_media(media)?;
    println!("Admitted {}", asset.name);
    Ok(())
}
