//! The person's Director choice: which connector and model S4 starts a Revise
//! cut run with. The choices are the connectors this build can launch
//! (`adapters::available_connectors`); nothing else can be chosen or used, and
//! a saved choice is never replaced by another.
#![allow(clippy::missing_errors_doc)]
use super::{adapters::available_connectors, orchestration_records::ProviderSelectionRecord};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

const DIRECTOR_FILE: &str = "director.json";
#[cfg(test)]
const DIRECTOR_FILE_BYTES: u64 = super::setting_file::MAX_BYTES;
const UNREADABLE: &str = "Your saved Director choice cannot be read. Choose a Director in Settings; your notes are saved and the choice has not been substituted.";
const NO_DIRECTOR: &str = "No Director provider is set up on this computer. Your notes are saved. Revise cut again after one is set up.";

/// The person's Director choice, in per-user scheduler settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectorSetting {
    pub version: u32,
    pub director: ProviderSelectionRecord,
}

/// What the Director setting holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectorStatus {
    Set(ProviderSelectionRecord),
    Missing,
    /// A choice this build does not offer. It is kept, never substituted;
    /// runs wait until the person picks another.
    NoLongerOffered(ProviderSelectionRecord),
    /// The file exists but cannot be used. It is never overwritten unasked.
    Unreadable(String),
}

/// Where S4 keeps per-user scheduler settings: beside Studio feedback state.
///
/// # Panics
/// Unit tests panic here to catch a host that omitted its isolated settings folder.
pub fn scheduler_settings_directory() -> Result<PathBuf, String> {
    #[cfg(test)]
    panic!("TE15_FIX_TEST_DEFAULT_SETTINGS");
    #[cfg(not(test))]
    Ok(kinewright_project::feedback_state_directory_with(
        |name| std::env::var_os(name),
        &std::env::temp_dir(),
    )?
    .with_file_name("scheduler"))
}

/// Every Director the person may choose: one per available connector.
#[must_use]
pub fn director_choices() -> Vec<ProviderSelectionRecord> {
    available_connectors()
        .iter()
        .map(|specification| specification.selection())
        .collect()
}

fn folder_or_default(folder: Option<&Path>) -> Result<PathBuf, String> {
    match folder {
        Some(folder) => Ok(folder.to_path_buf()),
        None => scheduler_settings_directory(),
    }
}

pub(super) fn valid_shape(director: &ProviderSelectionRecord) -> bool {
    !director.connector.is_empty()
        && !director.model.is_empty()
        && director.connector.len() <= 128
        && director.model.len() <= 256
        && director.options.len() <= 32
}

/// Read the setting. Only reads one bounded file; it never starts a provider.
#[must_use]
pub fn director_status(folder: Option<&Path>) -> DirectorStatus {
    director_status_in(folder, &director_choices())
}
fn director_status_in(
    folder: Option<&Path>,
    choices: &[ProviderSelectionRecord],
) -> DirectorStatus {
    let Ok(folder) = folder_or_default(folder) else {
        return DirectorStatus::Unreadable(UNREADABLE.into());
    };
    let path = folder.join(DIRECTOR_FILE);
    let setting = match super::setting_file::read::<DirectorSetting>(&path) {
        Ok(Some(setting)) if setting.version == 1 && valid_shape(&setting.director) => setting,
        Ok(None) => return DirectorStatus::Missing,
        _ => return DirectorStatus::Unreadable(UNREADABLE.into()),
    };
    if choices.contains(&setting.director) {
        DirectorStatus::Set(setting.director)
    } else {
        DirectorStatus::NoLongerOffered(setting.director)
    }
}

/// The configured Director provider, or why there is none, in plain words.
pub fn configured_director(folder: Option<&Path>) -> Result<ProviderSelectionRecord, String> {
    configured_director_in(folder, &director_choices())
}
fn configured_director_in(
    folder: Option<&Path>,
    choices: &[ProviderSelectionRecord],
) -> Result<ProviderSelectionRecord, String> {
    match director_status_in(folder, choices) {
        DirectorStatus::Set(director) => Ok(director),
        DirectorStatus::Missing => Err(NO_DIRECTOR.into()),
        DirectorStatus::NoLongerOffered(_) => Err("Your saved Director choice is no longer available. Choose another Director in Settings; your notes are saved and the choice has not been substituted.".into()),
        DirectorStatus::Unreadable(why) => Err(why),
    }
}

/// Choose the Director. Only an offered choice is written, whole or not at all:
/// a temporary file is synced, renamed over the setting, and its folder synced.
pub fn write_director_setting(
    folder: Option<&Path>,
    director: &ProviderSelectionRecord,
) -> Result<(), String> {
    write_director_setting_with(folder, director, |_| Ok(()))
}

fn write_director_setting_with(
    folder: Option<&Path>,
    director: &ProviderSelectionRecord,
    before_rename: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    write_director_setting_in(folder, director, &director_choices(), before_rename)
}
fn write_director_setting_in(
    folder: Option<&Path>,
    director: &ProviderSelectionRecord,
    choices: &[ProviderSelectionRecord],
    before_rename: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    if !choices.contains(director) {
        return Err(format!(
            "{} · {} is not a Director this computer offers.",
            director.connector, director.model
        ));
    }
    let folder = folder_or_default(folder)?;
    let bytes = serde_json::to_vec_pretty(&DirectorSetting {
        version: 1,
        director: director.clone(),
    })
    .map_err(|error| error.to_string())?;
    let failed = |error: std::io::Error| format!("The Director setting was not saved ({error}).");
    std::fs::create_dir_all(&folder).map_err(failed)?;
    let temp = folder.join(format!("{DIRECTOR_FILE}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        before_rename(&temp)?;
        std::fs::rename(&temp, folder.join(DIRECTOR_FILE))
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(failed(error));
    }
    sync_folder(&folder).map_err(failed)
}

#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub fn write_fixture_director_setting(folder: &Path) -> Result<(), String> {
    let choices: Vec<_> = super::adapters::fixture_connectors()
        .iter()
        .map(|s| s.selection())
        .collect();
    write_director_setting_in(Some(folder), &choices[0], &choices, |_| Ok(()))
}
#[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
pub(crate) fn configured_fixture_director(
    folder: Option<&Path>,
) -> Result<ProviderSelectionRecord, String> {
    let choices: Vec<_> = super::adapters::fixture_connectors()
        .iter()
        .map(|s| s.selection())
        .collect();
    configured_director_in(folder, &choices)
}

#[cfg(unix)]
fn sync_folder(folder: &Path) -> std::io::Result<()> {
    std::fs::File::open(folder)?.sync_all()
}
#[cfg(not(unix))]
#[allow(
    clippy::unnecessary_wraps,
    reason = "Windows renames are durable without a directory handle sync"
)]
fn sync_folder(_folder: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};

    fn fixture() -> ProviderSelectionRecord {
        super::super::adapters::fixture_connectors()
            .into_iter()
            .map(super::super::adapters::ConnectorSpecification::selection)
            .collect::<Vec<_>>()
            .into_iter()
            .next()
            .expect("fixture builds certify the scripted Director")
    }

    fn director_status(folder: Option<&Path>) -> DirectorStatus {
        super::director_status_in(folder, &[fixture()])
    }
    fn configured_director(folder: Option<&Path>) -> Result<ProviderSelectionRecord, String> {
        super::configured_director_in(folder, &[fixture()])
    }
    fn write_director_setting(
        folder: Option<&Path>,
        choice: &ProviderSelectionRecord,
    ) -> Result<(), String> {
        super::write_director_setting_in(folder, choice, &[fixture()], |_| Ok(()))
    }
    fn write_director_setting_with(
        folder: Option<&Path>,
        choice: &ProviderSelectionRecord,
        hook: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<(), String> {
        super::write_director_setting_in(folder, choice, &[fixture()], hook)
    }

    #[test]
    fn s4b_c5_product_registry_preserves_but_never_offers_fixture_choice() {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "c5-registry");
        super::write_fixture_director_setting(temp.root()).unwrap();
        let before = std::fs::read(temp.path("director.json")).unwrap();
        assert!(
            super::director_choices()
                .iter()
                .all(|choice| choice.connector != "scripted")
        );
        assert_eq!(
            super::director_status(Some(temp.root())),
            DirectorStatus::NoLongerOffered(fixture())
        );
        assert!(super::configured_director(Some(temp.root())).is_err());
        assert!(super::write_director_setting(Some(temp.root()), &fixture()).is_err());
        assert_eq!(std::fs::read(temp.path("director.json")).unwrap(), before);
    }

    /// The setting offers only certified connectors, writes whole or not at
    /// all, and leaves an unusable file for the person to replace.
    #[test]
    fn c10_director_setting_is_offered_atomic_and_never_overwritten() {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "c10-director");
        let folder = temp.path("scheduler");
        assert_eq!(
            director_status(Some(&folder)),
            DirectorStatus::Missing,
            "C10_MISSING"
        );
        assert!(configured_director(Some(&folder)).is_err(), "C10_MISSING");
        let chosen = fixture();
        assert_eq!(
            (chosen.connector.as_str(), chosen.model.as_str()),
            ("scripted", "fixture")
        );
        write_director_setting(Some(&folder), &chosen).unwrap();
        assert_eq!(
            director_status(Some(&folder)),
            DirectorStatus::Set(chosen.clone()),
            "C10_ROUND_TRIP"
        );
        assert_eq!(configured_director(Some(&folder)).unwrap(), chosen);
        let before = std::fs::read(folder.join(DIRECTOR_FILE)).unwrap();
        // A fault between the synced temporary file and the rename keeps the
        // old setting byte for byte and leaves no temporary file.
        let fault = write_director_setting_with(Some(&folder), &chosen, |_| {
            Err(std::io::Error::other("C10 injected fault before rename"))
        });
        assert!(fault.is_err(), "C10_FAULT");
        assert_eq!(
            std::fs::read(folder.join(DIRECTOR_FILE)).unwrap(),
            before,
            "C10_FAULT_KEEPS_OLD"
        );
        assert_eq!(
            std::fs::read_dir(&folder).unwrap().count(),
            1,
            "C10_FAULT_NO_TEMP"
        );
        // A chat harness is not a certified Director connector.
        let unoffered = ProviderSelectionRecord {
            connector: "codex".into(),
            model: "gpt-6".into(),
            options: vec![],
        };
        assert!(
            write_director_setting(Some(&folder), &unoffered).is_err(),
            "C10_UNOFFERED"
        );
        assert_eq!(
            std::fs::read(folder.join(DIRECTOR_FILE)).unwrap(),
            before,
            "C10_UNOFFERED"
        );
        // A setting written by an older build that offered another connector.
        std::fs::write(
            folder.join(DIRECTOR_FILE),
            serde_json::to_vec(&DirectorSetting {
                version: 1,
                director: unoffered.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            director_status(Some(&folder)),
            DirectorStatus::NoLongerOffered(unoffered),
            "C10_NO_LONGER_OFFERED"
        );
        assert!(
            configured_director(Some(&folder))
                .unwrap_err()
                .contains("no longer available"),
            "C10_NO_LONGER_OFFERED"
        );
        for (name, bytes) in [
            ("version", br#"{"version":2,"director":{"connector":"scripted","model":"fixture","options":[]}}"#.to_vec()),
            ("garbage", b"{not json".to_vec()),
            ("oversize", vec![b' '; usize::try_from(DIRECTOR_FILE_BYTES).unwrap() + 1]),
        ] {
            std::fs::write(folder.join(DIRECTOR_FILE), &bytes).unwrap();
            assert!(
                matches!(director_status(Some(&folder)), DirectorStatus::Unreadable(_)),
                "C10_UNREADABLE {name}"
            );
            assert_eq!(
                std::fs::read(folder.join(DIRECTOR_FILE)).unwrap(),
                bytes,
                "C10_UNREADABLE_UNTOUCHED {name}"
            );
        }
        // Choosing again replaces the unreadable file.
        write_director_setting(Some(&folder), &chosen).unwrap();
        assert_eq!(
            director_status(Some(&folder)),
            DirectorStatus::Set(chosen),
            "C10_CHOOSE_AGAIN"
        );
    }
}

#[cfg(test)]
#[test]
fn te15_b1_fix_test_hosts_cannot_read_default_folder() {
    assert!(
        std::panic::catch_unwind(scheduler_settings_directory).is_err(),
        "TE15_FIX_NO_PERSONAL_FOLDER"
    );
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn te15_b1_fix2_director_links_keep_an_unreadable_choice() {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "te15-director-link",
    );
    let entry = temp.path(DIRECTOR_FILE);
    let target = temp.path("target.json");
    std::os::unix::fs::symlink(&target, &entry).unwrap();
    for intact in [false, true] {
        if intact {
            std::fs::write(
                &target,
                serde_json::to_vec(&DirectorSetting {
                    version: 1,
                    director: director_choices()[0].clone(),
                })
                .unwrap(),
            )
            .unwrap();
        }
        assert_eq!(
            director_status(Some(temp.root())),
            DirectorStatus::Unreadable(UNREADABLE.into()),
            "TE15_FIX2_DIRECTOR_LINK"
        );
        assert_eq!(
            configured_director(Some(temp.root())).unwrap_err(),
            UNREADABLE,
            "TE15_FIX2_DIRECTOR_WORDS"
        );
        assert!(
            std::fs::symlink_metadata(&entry)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn te15_b1_fix2_director_fifo_is_bounded() {
    const CHILD: &str = "KINEWRIGHT_TE15_DIRECTOR_FIFO";
    if let Some(folder) = std::env::var_os(CHILD) {
        assert!(
            matches!(
                director_status(Some(Path::new(&folder))),
                DirectorStatus::Unreadable(_)
            ),
            "TE15_FIX2_DIRECTOR_FIFO_REFUSED"
        );
        return;
    }
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "te15-director-fifo",
    );
    assert!(
        std::process::Command::new("mkfifo")
            .arg(temp.path(DIRECTOR_FILE))
            .status()
            .unwrap()
            .success()
    );
    let output_path = temp.path("child.log");
    let output = std::fs::File::create(&output_path).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "orchestration::director_setting::te15_b1_fix2_director_fifo_is_bounded",
            "--nocapture",
        ])
        .env(CHILD, temp.root())
        .stdout(output.try_clone().unwrap())
        .stderr(output)
        .spawn()
        .unwrap();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let result = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() >= end {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(
        result.is_some_and(|s| s.success()),
        "TE15_FIX2_DIRECTOR_FIFO_BOUNDED: {}",
        std::fs::read_to_string(output_path).unwrap()
    );
}
