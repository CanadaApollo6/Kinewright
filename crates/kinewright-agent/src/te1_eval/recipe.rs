//! Offline recipe sizing. This never constructs a provider or changes the cap.
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use serde_json::Value;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeSize {
    pub total_bytes: usize,
    pub identified_run_string_bytes: usize,
    pub longer_home_bytes: usize,
}
#[cfg(target_os = "linux")]
pub(crate) fn measure(program: &Value, run: &Value) -> RecipeSize {
    let text = serde_json::to_string(program).unwrap();
    let mut strings = Vec::new();
    collect(run, &mut strings, false);
    collect(program, &mut strings, true);
    strings.sort_by_key(|s| std::cmp::Reverse(s.len()));
    strings.dedup();
    let mut variable = 0;
    let mut offset = 0;
    while offset < text.len() {
        if let Some(s) = strings
            .iter()
            .find(|s| text.as_bytes()[offset..].starts_with(s.as_bytes()))
        {
            offset += s.len();
            variable += s.len();
        } else {
            offset += 1;
        }
    }
    let home = strings
        .iter()
        .find_map(|s| s.strip_prefix("/home/"))
        .and_then(|p| p.split('/').next())
        .map(|name| format!("/home/{name}/"));
    RecipeSize {
        total_bytes: text.len(),
        identified_run_string_bytes: variable,
        longer_home_bytes: home.map_or(text.len(), |home| {
            text.replace(&home, &format!("/home/{}/", "x".repeat(32)))
                .len()
        }),
    }
}
#[cfg(target_os = "linux")]
fn collect(value: &Value, strings: &mut Vec<String>, paths_only: bool) {
    match value {
        Value::String(s)
            if !s.is_empty()
                && (s.starts_with("/home/")
                    || s.starts_with("http://127.0.0.1:")
                    || (!paths_only
                        && [32, 64].contains(&s.len())
                        && s.bytes().all(|b| b.is_ascii_hexdigit()))) =>
        {
            strings.push(s.clone());
        }
        Value::Object(map) => {
            for value in map.values() {
                collect(value, strings, paths_only);
            }
        }
        Value::Array(array) => {
            for value in array {
                collect(value, strings, paths_only);
            }
        }
        _ => {}
    }
}
