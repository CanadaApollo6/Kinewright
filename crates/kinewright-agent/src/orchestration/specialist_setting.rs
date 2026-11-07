//! Person-owned specialist preference. Assignment snapshots it into the task brief;
//! replay and running tasks keep that snapshot, even after Settings changes.
#![allow(clippy::missing_errors_doc)]
use super::{
    adapters::available_connectors,
    director_setting::scheduler_settings_directory,
    orchestration_records::{ProviderOptionRecord, ProviderSelectionRecord},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
};

const FILE: &str = "specialists.json";
const MAX_BYTES: u64 = 16 << 10;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "selection",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SpecialistChoice {
    #[default]
    SameAsDirector,
    Specific(ProviderSelectionRecord),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecialistStatus {
    Set(SpecialistChoice),
    NoLongerOffered(ProviderSelectionRecord),
    Unreadable(String),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Setting {
    version: u32,
    specialists: SpecialistChoice,
}

/// Explicit native model/option combinations. No default-model alias, discovery
/// request or credential read. Ultracode would enable delegation and is excluded.
#[must_use]
pub fn specialist_choices() -> Vec<ProviderSelectionRecord> {
    let mut choices = Vec::new();
    for spec in available_connectors() {
        let default = spec.selection();
        choices.push(default.clone());
        let models = match spec.provider() {
            "claude" => crate::claude_models(),
            "codex" => crate::codex_models(),
            _ => continue,
        };
        for model in models {
            if model.id.is_empty()
                || model.id.len() > 256
                || !model
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
            {
                continue;
            }
            for effort in model.efforts.iter().filter(|effort| {
                matches!(
                    effort.as_str(),
                    "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "none"
                )
            }) {
                let mut tiers = vec![None];
                if spec.provider() == "codex"
                    && model.tiers.iter().any(|tier| tier.id == "priority")
                {
                    tiers.push(Some("priority"));
                }
                for tier in tiers {
                    let mut options = vec![ProviderOptionRecord {
                        name: if spec.provider() == "codex" {
                            "model_reasoning_effort"
                        } else {
                            "effort"
                        }
                        .into(),
                        value: effort.clone(),
                    }];
                    if let Some(tier) = tier {
                        options.push(ProviderOptionRecord {
                            name: "service_tier".into(),
                            value: tier.into(),
                        });
                    }
                    let choice = ProviderSelectionRecord {
                        connector: default.connector.clone(),
                        model: model.id.clone(),
                        options,
                    };
                    if !choices.contains(&choice) {
                        choices.push(choice);
                    }
                }
            }
        }
    }
    choices
}

/// Missing means the visible Same as Director choice. Broken or unavailable
/// saved choices are retained and refuse; they never fall back to inheritance.
#[must_use]
pub fn specialist_status(folder: Option<&Path>) -> SpecialistStatus {
    let read = || -> Result<Option<Setting>, String> {
        let folder = folder.map_or_else(scheduler_settings_directory, |p| Ok(p.to_path_buf()))?;
        let file = match std::fs::File::open(folder.join(FILE)) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Specialist setting is too large.".into());
        }
        let setting: Setting = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if setting.version != 1 {
            return Err("Unknown specialist setting version.".into());
        }
        Ok(Some(setting))
    };
    match read() {
        Ok(None) => SpecialistStatus::Set(SpecialistChoice::SameAsDirector),
        Ok(Some(Setting {
            specialists: SpecialistChoice::Specific(selection),
            ..
        })) if !specialist_choices().contains(&selection) => {
            SpecialistStatus::NoLongerOffered(selection)
        }
        Ok(Some(setting)) => SpecialistStatus::Set(setting.specialists),
        Err(why) => {
            SpecialistStatus::Unreadable(format!("The specialist setting cannot be read ({why})."))
        }
    }
}

pub fn configured_specialist(
    folder: Option<&Path>,
    director: &ProviderSelectionRecord,
) -> Result<ProviderSelectionRecord, String> {
    match specialist_status(folder) {
        SpecialistStatus::Set(SpecialistChoice::SameAsDirector) => Ok(director.clone()),
        SpecialistStatus::Set(SpecialistChoice::Specific(selection)) => Ok(selection),
        SpecialistStatus::NoLongerOffered(selection) => Err(format!(
            "Your specialist choice ({} · {}) is unavailable. Choose another in Settings; it has not been substituted.",
            selection.connector, selection.model
        )),
        SpecialistStatus::Unreadable(why) => Err(why),
    }
}

/// Save the explicit choice atomically, without touching director.json.
pub fn write_specialist_setting(
    folder: Option<&Path>,
    choice: &SpecialistChoice,
) -> Result<(), String> {
    if let SpecialistChoice::Specific(selection) = choice
        && !specialist_choices().contains(selection)
    {
        return Err("This specialist model or its options are unavailable.".into());
    }
    let folder = folder.map_or_else(scheduler_settings_directory, |p| Ok(p.to_path_buf()))?;
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&Setting {
        version: 1,
        specialists: choice.clone(),
    })
    .map_err(|e| e.to_string())?;
    let temp = folder.join(format!("{FILE}.{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, folder.join(FILE))?;
        #[cfg(unix)]
        std::fs::File::open(&folder)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result.map_err(|e| format!("The specialist setting was not saved ({e})."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    fn director(model: &str) -> ProviderSelectionRecord {
        ProviderSelectionRecord {
            connector: "scripted".into(),
            model: model.into(),
            options: vec![],
        }
    }
    #[test]
    fn te15_b1_missing_is_explicit_inheritance_and_tracks_director() {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "te15-specialists-default");
        assert_eq!(
            specialist_status(Some(temp.root())),
            SpecialistStatus::Set(SpecialistChoice::SameAsDirector)
        );
        for model in ["first", "changed"] {
            assert_eq!(
                configured_specialist(Some(temp.root()), &director(model)).unwrap(),
                director(model)
            );
        }
        write_specialist_setting(Some(temp.root()), &SpecialistChoice::SameAsDirector).unwrap();
        assert_eq!(
            configured_specialist(Some(temp.root()), &director("later")).unwrap(),
            director("later")
        );
    }
    #[test]
    fn te15_b1_unavailable_and_broken_choices_are_preserved_and_refuse() {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "te15-specialists-refuse");
        for bytes in [
            serde_json::to_vec(&Setting {
                version: 1,
                specialists: SpecialistChoice::Specific(director("retired")),
            })
            .unwrap(),
            b"broken".to_vec(),
            vec![b'x'; usize::try_from(MAX_BYTES).unwrap() + 1],
        ] {
            std::fs::write(temp.path(FILE), &bytes).unwrap();
            assert!(
                configured_specialist(Some(temp.root()), &director("other")).is_err(),
                "TE15_SPECIALIST_REFUSAL"
            );
            assert_eq!(std::fs::read(temp.path(FILE)).unwrap(), bytes);
        }
        assert!(
            write_specialist_setting(
                Some(temp.root()),
                &SpecialistChoice::Specific(director("retired"))
            )
            .is_err()
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn te15_b1_specific_options_round_trip_without_touching_director() {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "te15-specialists-specific");
        std::fs::write(temp.path("director.json"), b"Director bytes").unwrap();
        let selection = specialist_choices()
            .into_iter()
            .find(|s| s.model == "claude-sonnet-5-5" && s.options[0].value == "low")
            .unwrap();
        write_specialist_setting(
            Some(temp.root()),
            &SpecialistChoice::Specific(selection.clone()),
        )
        .unwrap();
        assert_eq!(
            configured_specialist(Some(temp.root()), &director("other")).unwrap(),
            selection
        );
        assert_eq!(
            std::fs::read(temp.path("director.json")).unwrap(),
            b"Director bytes"
        );
        assert!(
            specialist_choices()
                .iter()
                .all(|s| s.options.iter().all(|v| v.value != crate::CLAUDE_ULTRACODE))
        );
    }
}
