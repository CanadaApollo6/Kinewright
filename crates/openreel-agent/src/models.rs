//! Model choices for the supported harnesses.
//!
//! Claude Code exposes its standard tier aliases, which the CLI resolves to
//! the current model of each tier. Codex choices come from the CLI's own
//! model cache, so the list always matches what the installed CLI can run;
//! no cache means no named choices, and the picker degrades to the CLI's
//! configured default.

use std::fs;

use serde_json::Value;

use crate::drivers::codex_model_cache_path;

/// One selectable model: the id passed to the CLI and a display label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
}

impl ModelChoice {
    fn new(id: &str, label: &str) -> Self {
        Self {
            id: id.to_owned(),
            label: label.to_owned(),
        }
    }
}

/// The Claude Code tier aliases, resolved by the CLI to its current models.
#[must_use]
pub fn claude_models() -> Vec<ModelChoice> {
    vec![
        ModelChoice::new("opus", "Opus"),
        ModelChoice::new("sonnet", "Sonnet"),
        ModelChoice::new("haiku", "Haiku"),
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

/// The cache is either a bare model array or an object with a `models` array;
/// entries carry a `slug` (the `-m` argument) and usually a `display_name`.
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
            Some(ModelChoice::new(slug, label))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_tiers_are_the_cli_aliases() {
        let ids: Vec<_> = claude_models().into_iter().map(|model| model.id).collect();
        assert_eq!(ids, ["opus", "sonnet", "haiku"]);
    }

    #[test]
    fn codex_catalog_parses_both_shapes_and_falls_back_to_slug() {
        let object_form = r#"{"models": [
            {"slug": "gpt-5.6-sol", "display_name": "GPT-5.6-Sol"},
            {"slug": "gpt-5.5"}
        ]}"#;
        assert_eq!(
            parse_codex_models(object_form),
            vec![
                ModelChoice::new("gpt-5.6-sol", "GPT-5.6-Sol"),
                ModelChoice::new("gpt-5.5", "gpt-5.5"),
            ]
        );
        let array_form = r#"[{"slug": "gpt-5.4-mini", "display_name": "GPT-5.4-Mini"}]"#;
        assert_eq!(
            parse_codex_models(array_form),
            vec![ModelChoice::new("gpt-5.4-mini", "GPT-5.4-Mini")]
        );
    }

    #[test]
    fn malformed_or_empty_catalogs_yield_no_choices() {
        assert!(parse_codex_models("not json").is_empty());
        assert!(parse_codex_models("{}").is_empty());
        assert!(parse_codex_models(r#"{"models": [{"no_slug": true}]}"#).is_empty());
    }
}
