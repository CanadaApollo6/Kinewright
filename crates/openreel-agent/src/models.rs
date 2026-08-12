//! Model and effort choices for the supported harnesses.
//!
//! Claude Code exposes its standard tier aliases, which the CLI resolves to
//! the current model of each tier. Codex choices come from the CLI's own
//! model cache, so the list always matches what the installed CLI can run;
//! no cache means no named choices, and the picker degrades to the CLI's
//! configured default. Every model carries the reasoning-effort levels it
//! supports (Claude's are the CLI's session levels; Codex's come from the
//! same cache), so an effort is only ever offered where it is valid.

use std::fs;

use serde_json::Value;

use crate::drivers::codex_model_cache_path;

/// The `claude` CLI's session effort levels (`--effort`).
const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// One selectable model: the id passed to the CLI, a display label, and the
/// reasoning-effort levels the model supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
    pub efforts: Vec<String>,
}

impl ModelChoice {
    fn new(id: &str, label: &str, efforts: &[&str]) -> Self {
        Self {
            id: id.to_owned(),
            label: label.to_owned(),
            efforts: efforts.iter().map(|&effort| effort.to_owned()).collect(),
        }
    }
}

/// The Claude Code tier aliases, resolved by the CLI to its current models.
#[must_use]
pub fn claude_models() -> Vec<ModelChoice> {
    vec![
        ModelChoice::new("opus", "Opus", &CLAUDE_EFFORTS),
        ModelChoice::new("sonnet", "Sonnet", &CLAUDE_EFFORTS),
        ModelChoice::new("haiku", "Haiku", &CLAUDE_EFFORTS),
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

/// The cache is either a bare model array or an object with a `models` array;
/// entries carry a `slug` (the `-m` argument), usually a `display_name`, and
/// a `supported_reasoning_levels` array of `{effort, description}` objects.
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
            Some(ModelChoice {
                id: slug.to_owned(),
                label: label.to_owned(),
                efforts,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_tiers_are_the_cli_aliases_with_its_session_efforts() {
        let models = claude_models();
        let ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["opus", "sonnet", "haiku"]);
        for model in &models {
            assert_eq!(model.efforts, CLAUDE_EFFORTS);
        }
    }

    #[test]
    fn codex_catalog_parses_both_shapes_efforts_and_slug_fallback() {
        let object_form = r#"{"models": [
            {
                "slug": "gpt-5.6-sol",
                "display_name": "GPT-5.6-Sol",
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "fast"},
                    {"effort": "xhigh", "description": "deep"}
                ]
            },
            {"slug": "gpt-5.5"}
        ]}"#;
        assert_eq!(
            parse_codex_models(object_form),
            vec![
                ModelChoice::new("gpt-5.6-sol", "GPT-5.6-Sol", &["low", "xhigh"]),
                ModelChoice::new("gpt-5.5", "gpt-5.5", &[]),
            ]
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
}
