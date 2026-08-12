//! Model and effort choices for the supported harnesses.
//!
//! Claude Code choices are a curated list of the current versioned model
//! names, since its CLI keeps no on-disk catalog. Codex choices come from its
//! model cache, so the list always matches what the installed CLI can run;
//! no cache means no named choices, and the picker degrades to the CLI's
//! configured default. Every model carries the reasoning-effort levels it
//! supports (Claude's are the CLI's session levels; Codex's come from the
//! same cache), so an effort is only ever offered where it is valid.

use std::fs;

use serde_json::Value;

use crate::drivers::{codex_config_path, codex_model_cache_path};

/// Not an `--effort` value: Ultracode is the `claude` CLI's session setting
/// for xhigh effort plus standing dynamic-workflow (multi-agent)
/// orchestration, passed as `--settings {"ultracode": true}`. It sits in the
/// effort list because users reach for it as the tier above max, and it is
/// only offered on xhigh-capable models, which the CLI requires.
pub const CLAUDE_ULTRACODE: &str = "ultracode";

/// The `claude` CLI's full session effort ladder (`--effort` plus the
/// Ultracode session setting). Which rungs a model supports varies: the
/// CLI's own capability checks deny `xhigh` to everything before the
/// Opus/Sonnet 4.7 generation and deny `max` to everything before 4.6, so
/// each curated model carries its true subset.
const CLAUDE_EFFORTS_FULL: [&str; 6] = ["low", "medium", "high", "xhigh", "max", CLAUDE_ULTRACODE];
/// The 4.6 generation: `max` exists, `xhigh` (and so Ultracode) does not.
const CLAUDE_EFFORTS_NO_XHIGH: [&str; 4] = ["low", "medium", "high", "max"];
/// Older models (Haiku 4.5 era): the base three levels only.
const CLAUDE_EFFORTS_BASE: [&str; 3] = ["low", "medium", "high"];

/// One service tier a model can run under: the id passed to the CLI and the
/// name shown in the picker (e.g. Codex's `priority` tier displays as "Fast").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceTier {
    pub id: String,
    pub name: String,
}

/// One selectable model: the id passed to the CLI, a display label, the
/// reasoning-effort levels the model supports, and any faster-than-standard
/// service tiers it offers (empty for providers without tiers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
    pub efforts: Vec<String>,
    pub tiers: Vec<ServiceTier>,
}

impl ModelChoice {
    fn new(id: &str, label: &str, efforts: &[&str]) -> Self {
        Self {
            id: id.to_owned(),
            label: label.to_owned(),
            efforts: efforts.iter().map(|&effort| effort.to_owned()).collect(),
            tiers: Vec::new(),
        }
    }
}

/// The current Claude models by full versioned name, newest tier first.
///
/// The `claude` CLI has no on-disk model catalog to read the way Codex does,
/// so this list is curated from the models the installed CLI accepts; ids are
/// full names (`claude-opus-5`) rather than tier aliases so users can pin a
/// version, e.g. Opus 4.8 versus Opus 5.
#[must_use]
pub fn claude_models() -> Vec<ModelChoice> {
    vec![
        ModelChoice::new("claude-fable-5", "Fable 5", &CLAUDE_EFFORTS_FULL),
        ModelChoice::new("claude-opus-5", "Opus 5", &CLAUDE_EFFORTS_FULL),
        ModelChoice::new("claude-opus-4-8", "Opus 4.8", &CLAUDE_EFFORTS_FULL),
        ModelChoice::new("claude-sonnet-5", "Sonnet 5", &CLAUDE_EFFORTS_FULL),
        ModelChoice::new("claude-sonnet-4-6", "Sonnet 4.6", &CLAUDE_EFFORTS_NO_XHIGH),
        ModelChoice::new("claude-haiku-4-5", "Haiku 4.5", &CLAUDE_EFFORTS_BASE),
    ]
}

/// Models the installed Codex CLI advertises, from its on-disk model cache.
#[must_use]
pub fn codex_models() -> Vec<ModelChoice> {
    codex_model_cache_path()
        .filter(|path| path.is_file())
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|source| parse_codex_models(&source))
        .unwrap_or_default()
}

/// The model slug the installed Codex CLI runs when none is passed, from the
/// top-level `model = "..."` entry in its `config.toml`. This is what the
/// picker's "Default" really means, so effort and speed options for Default
/// can be the default model's own instead of a lowest-common-denominator set.
#[must_use]
pub fn codex_default_model() -> Option<String> {
    codex_config_path()
        .filter(|path| path.is_file())
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|source| parse_codex_default_model(&source))
}

/// Only the top-level `model` key counts: table sections (`[section]`) can
/// carry their own `model` keys with different meanings.
fn parse_codex_default_model(source: &str) -> Option<String> {
    for line in source.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            return None;
        }
        if let Some(value) = line.strip_prefix("model") {
            let value = value.trim_start();
            if let Some(value) = value.strip_prefix('=') {
                let value = value.trim();
                let value = value.strip_prefix('"').unwrap_or(value);
                let value = value.strip_suffix('"').unwrap_or(value);
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
    }
    None
}

/// Effort levels every model in the catalog supports, in the first model's
/// order - the only levels safe to pass when the model itself is the CLI's
/// default and therefore unknown.
#[must_use]
pub fn common_efforts(models: &[ModelChoice]) -> Vec<String> {
    let Some((first, rest)) = models.split_first() else {
        return Vec::new();
    };
    first
        .efforts
        .iter()
        .filter(|effort| rest.iter().all(|model| model.efforts.contains(effort)))
        .cloned()
        .collect()
}

/// Service tiers every model in the catalog supports, in the first model's
/// order - the safe set when the model is the CLI's default and unknown.
#[must_use]
pub fn common_tiers(models: &[ModelChoice]) -> Vec<ServiceTier> {
    let Some((first, rest)) = models.split_first() else {
        return Vec::new();
    };
    first
        .tiers
        .iter()
        .filter(|tier| {
            rest.iter()
                .all(|model| model.tiers.iter().any(|other| other.id == tier.id))
        })
        .cloned()
        .collect()
}

/// The cache is either a bare model array or an object with a `models` array;
/// entries carry a `slug` (the `-m` argument), usually a `display_name`, a
/// `supported_reasoning_levels` array of `{effort, description}` objects, and
/// optionally a `service_tiers` array of `{id, name, description}` objects
/// for faster-than-standard tiers (the `service_tier` config value).
fn parse_codex_models(source: &str) -> Vec<ModelChoice> {
    let Ok(catalog) = serde_json::from_str::<Value>(source) else {
        return Vec::new();
    };
    let models = if catalog.is_array() {
        catalog.as_array()
    } else {
        catalog.get("models").and_then(Value::as_array)
    };
    models
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let slug = model.get("slug").and_then(Value::as_str)?;
            let label = model
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or(slug);
            let efforts = model
                .get("supported_reasoning_levels")
                .and_then(Value::as_array)
                .map(|levels| {
                    levels
                        .iter()
                        .filter_map(|level| level.get("effort").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let tiers = model
                .get("service_tiers")
                .and_then(Value::as_array)
                .map(|tiers| {
                    tiers
                        .iter()
                        .filter_map(|tier| {
                            let id = tier.get("id").and_then(Value::as_str)?;
                            let name = tier.get("name").and_then(Value::as_str).unwrap_or(id);
                            Some(ServiceTier {
                                id: id.to_owned(),
                                name: name.to_owned(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(ModelChoice {
                id: slug.to_owned(),
                label: label.to_owned(),
                efforts,
                tiers,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_models_are_versioned_full_names_with_per_model_efforts() {
        let models = claude_models();
        let ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "claude-fable-5",
                "claude-opus-5",
                "claude-opus-4-8",
                "claude-sonnet-5",
                "claude-sonnet-4-6",
                "claude-haiku-4-5",
            ]
        );
        let efforts_of = |id: &str| {
            models
                .iter()
                .find(|model| model.id == id)
                .map(|model| model.efforts.clone())
                .unwrap()
        };
        // Per the CLI's own capability checks: the 4.7+ generation takes the
        // full ladder, 4.6 lacks xhigh, Haiku 4.5 stops at high.
        assert_eq!(efforts_of("claude-fable-5"), CLAUDE_EFFORTS_FULL);
        assert_eq!(efforts_of("claude-opus-4-8"), CLAUDE_EFFORTS_FULL);
        assert_eq!(efforts_of("claude-sonnet-4-6"), CLAUDE_EFFORTS_NO_XHIGH);
        assert_eq!(efforts_of("claude-haiku-4-5"), CLAUDE_EFFORTS_BASE);
    }

    #[test]
    fn codex_catalog_parses_both_shapes_efforts_tiers_and_slug_fallback() {
        let object_form = r#"{"models": [
            {
                "slug": "gpt-5.6-sol",
                "display_name": "GPT-5.6-Sol",
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "fast"},
                    {"effort": "xhigh", "description": "deep"}
                ],
                "service_tiers": [
                    {"id": "priority", "name": "Fast", "description": "1.5x speed"}
                ]
            },
            {"slug": "gpt-5.5"}
        ]}"#;
        let mut sol = ModelChoice::new("gpt-5.6-sol", "GPT-5.6-Sol", &["low", "xhigh"]);
        sol.tiers = vec![ServiceTier {
            id: "priority".to_owned(),
            name: "Fast".to_owned(),
        }];
        assert_eq!(
            parse_codex_models(object_form),
            vec![sol, ModelChoice::new("gpt-5.5", "gpt-5.5", &[])]
        );
        let array_form = r#"[{"slug": "gpt-5.4-mini", "display_name": "GPT-5.4-Mini"}]"#;
        assert_eq!(
            parse_codex_models(array_form),
            vec![ModelChoice::new("gpt-5.4-mini", "GPT-5.4-Mini", &[])]
        );
    }

    #[test]
    fn malformed_or_empty_catalogs_yield_no_choices() {
        assert!(parse_codex_models("not json").is_empty());
        assert!(parse_codex_models("{}").is_empty());
        assert!(parse_codex_models(r#"{"models": [{"no_slug": true}]}"#).is_empty());
    }

    #[test]
    fn common_efforts_is_the_ordered_intersection() {
        let models = vec![
            ModelChoice::new("a", "A", &["low", "medium", "high", "ultra"]),
            ModelChoice::new("b", "B", &["medium", "low", "high"]),
        ];
        assert_eq!(common_efforts(&models), ["low", "medium", "high"]);
        assert!(common_efforts(&[]).is_empty());
    }

    #[test]
    fn codex_default_model_reads_only_the_top_level_key() {
        let config = "model = \"gpt-5.6-sol\"\nmodel_reasoning_effort = \"xhigh\"\n";
        assert_eq!(
            parse_codex_default_model(config),
            Some("gpt-5.6-sol".to_owned())
        );
        // `model` keys inside sections do not count, and prefixed keys like
        // model_reasoning_effort never match.
        let sectioned = "model_reasoning_effort = \"xhigh\"\n[profile]\nmodel = \"gpt-5.5\"\n";
        assert_eq!(parse_codex_default_model(sectioned), None);
        assert_eq!(parse_codex_default_model(""), None);
    }

    #[test]
    fn common_tiers_is_the_ordered_intersection_by_id() {
        let tier = |id: &str, name: &str| ServiceTier {
            id: id.to_owned(),
            name: name.to_owned(),
        };
        let mut a = ModelChoice::new("a", "A", &[]);
        a.tiers = vec![tier("priority", "Fast"), tier("flex", "Flex")];
        let mut b = ModelChoice::new("b", "B", &[]);
        b.tiers = vec![tier("priority", "Fast")];
        let models = vec![a, b];
        assert_eq!(common_tiers(&models), vec![tier("priority", "Fast")]);
        assert!(common_tiers(&[]).is_empty());
        let untiered = vec![ModelChoice::new("c", "C", &[])];
        assert!(common_tiers(&untiered).is_empty());
    }
}
