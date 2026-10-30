//! The person's Director choice: which certified connector and model S4 starts
//! a Revise cut run with. The choices are the certified connector registry's
//! (`adapters::certified_connectors`); nothing else can be chosen or used.
#![allow(clippy::missing_errors_doc)]
use super::{adapters::certified_connectors, orchestration_records::ProviderSelectionRecord};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

const DIRECTOR_FILE: &str = "director.json";
const DIRECTOR_FILE_BYTES: u64 = 16 << 10;
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
    /// A choice this build no longer certifies. Runs wait until another is chosen.
    NoLongerOffered(ProviderSelectionRecord),
    /// The file exists but cannot be used. It is never overwritten unasked.
    Unreadable(String),
}

/// Where S4 keeps per-user scheduler settings: beside Studio feedback state.
pub fn scheduler_settings_directory() -> Result<PathBuf, String> {
    Ok(kinewright_project::feedback_state_directory_with(
        |name| std::env::var_os(name),
        &std::env::temp_dir(),
    )?
    .with_file_name("scheduler"))
}

/// Every Director the person may choose: one per qualified release specification.
#[must_use]
pub fn director_choices() -> Vec<ProviderSelectionRecord> {
    certified_connectors()
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

fn valid_shape(director: &ProviderSelectionRecord) -> bool {
    !director.connector.is_empty()
        && !director.model.is_empty()
        && director.connector.len() <= 128
        && director.model.len() <= 256
        && director.options.len() <= 32
}

/// Read the setting. Only reads one bounded file; it never starts a provider.
#[must_use]
pub fn director_status(folder: Option<&Path>) -> DirectorStatus {
    let folder = match folder_or_default(folder) {
        Ok(folder) => folder,
        Err(why) => return DirectorStatus::Unreadable(why),
    };
    let path = folder.join(DIRECTOR_FILE);
    let bytes = match std::fs::metadata(&path) {
        Ok(metadata) if metadata.len() > DIRECTOR_FILE_BYTES => {
            return DirectorStatus::Unreadable("The Director setting is too large to read.".into());
        }
        Ok(_) => match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return DirectorStatus::Unreadable(format!(
                    "The Director setting cannot be read ({error})."
                ));
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return DirectorStatus::Missing;
        }
        Err(error) => {
            return DirectorStatus::Unreadable(format!(
                "The Director setting cannot be read ({error})."
            ));
        }
    };
    let setting: DirectorSetting = match serde_json::from_slice(&bytes) {
        Ok(setting) => setting,
        Err(error) => {
            return DirectorStatus::Unreadable(format!(
                "The Director setting cannot be read ({error})."
            ));
        }
    };
    if setting.version != 1 || !valid_shape(&setting.director) {
        return DirectorStatus::Unreadable(
            "The Director setting has an unknown version or shape.".into(),
        );
    }
    if director_choices().contains(&setting.director) {
        DirectorStatus::Set(setting.director)
    } else {
        DirectorStatus::NoLongerOffered(setting.director)
    }
}

/// The configured Director provider, or why there is none, in plain words.
pub fn configured_director(folder: Option<&Path>) -> Result<ProviderSelectionRecord, String> {
    if folder.is_none()
        && let Err(why) = scheduler_settings_directory()
    {
        return Err(format!("{NO_DIRECTOR} ({why})"));
    }
    match director_status(folder) {
        DirectorStatus::Set(director) => Ok(director),
        DirectorStatus::Missing => Err(NO_DIRECTOR.into()),
        DirectorStatus::NoLongerOffered(director) => Err(format!(
            "The Director you chose ({} · {}) is no longer available on this computer. Your notes are saved. Choose another Director in Settings, then Revise cut again.",
            director.connector, director.model
        )),
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
    if !director_choices().contains(director) {
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
        director_choices()
            .into_iter()
            .next()
            .expect("fixture builds certify the scripted Director")
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
