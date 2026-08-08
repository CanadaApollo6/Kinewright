use std::collections::{HashMap, HashSet};

use openreel_core::{AgentError, AgentEvent};
use serde_json::Value;

#[derive(Default)]
pub(crate) struct ClaudeProtocol {
    tool_names: HashMap<String, String>,
}

impl ClaudeProtocol {
    pub(crate) fn parse_line(&mut self, line: &str) -> Result<Vec<AgentEvent>, AgentError> {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| AgentError::Protocol(format!("invalid Claude stream JSON: {error}")))?;
        let mut events = Vec::new();
        match value.get("type").and_then(Value::as_str) {
            Some("assistant") => self.parse_assistant(&value, &mut events),
            Some("user") => self.parse_tool_results(&value, &mut events),
            Some("result") => parse_claude_result(&value, &mut events),
            Some("system") | Some("stream_event") | Some("rate_limit_event") => {}
            Some(other) => {
                if value.get("error").is_some() {
                    events.push(AgentEvent::Text(format!(
                        "Claude {other}: {}",
                        compact_json(value.get("error").unwrap_or(&Value::Null))
                    )));
                }
            }
            None => {}
        }
        Ok(events)
    }

    fn parse_assistant(&mut self, value: &Value, events: &mut Vec<AgentEvent>) {
        let Some(content) = value
            .pointer("/message/content")
            .and_then(Value::as_array)
        else {
            return;
        };
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str)
                        && !text.is_empty()
                    {
                        events.push(AgentEvent::Text(text.to_owned()));
                    }
                }
                Some("tool_use") => {
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let raw_name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let name = display_tool_name(raw_name);
                    if !id.is_empty() {
                        self.tool_names.insert(id.to_owned(), name.clone());
                    }
                    events.push(AgentEvent::ToolCall {
                        name,
                        arguments: compact_json(block.get("input").unwrap_or(&Value::Null)),
                    });
                }
                _ => {}
            }
        }
    }

    fn parse_tool_results(&mut self, value: &Value, events: &mut Vec<AgentEvent>) {
        let Some(content) = value
            .pointer("/message/content")
            .and_then(Value::as_array)
        else {
            return;
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let id = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = self
                .tool_names
                .remove(id)
                .unwrap_or_else(|| "tool".to_owned());
            events.push(AgentEvent::ToolResult {
                name,
                result: content_text(block.get("content").unwrap_or(&Value::Null)),
            });
        }
    }
}

fn parse_claude_result(value: &Value, events: &mut Vec<AgentEvent>) {
    if value.get("is_error").and_then(Value::as_bool) == Some(true)
        && let Some(result) = value.get("result").and_then(Value::as_str)
        && !result.is_empty()
    {
        events.push(AgentEvent::Text(format!("Claude error: {result}")));
    }
    let usage = value.get("usage").unwrap_or(&Value::Null);
    let input_tokens = token_value(usage, "input_tokens", "inputTokens")
        .or_else(|| model_usage_total(value, "inputTokens"))
        .unwrap_or(0);
    let output_tokens = token_value(usage, "output_tokens", "outputTokens")
        .or_else(|| model_usage_total(value, "outputTokens"))
        .unwrap_or(0);
    events.push(AgentEvent::Cost {
        input_tokens,
        output_tokens,
        cost_usd: value.get("total_cost_usd").and_then(Value::as_f64),
    });
    events.push(AgentEvent::Done);
}

fn model_usage_total(value: &Value, key: &str) -> Option<u64> {
    let usage = value
        .get("modelUsage")
        .or_else(|| value.get("model_usage"))?
        .as_object()?;
    Some(
        usage
            .values()
            .filter_map(|model| model.get(key).and_then(Value::as_u64))
            .sum(),
    )
}

// Kept and fixture-tested for the moment Codex can safely restrict built-in tools.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Default)]
pub(crate) struct CodexProtocol {
    tool_names: HashMap<String, String>,
    announced: HashSet<String>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CodexProtocol {
    pub(crate) fn parse_line(&mut self, line: &str) -> Result<Vec<AgentEvent>, AgentError> {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| AgentError::Protocol(format!("invalid Codex JSONL: {error}")))?;
        let mut events = Vec::new();
        match value.get("type").and_then(Value::as_str) {
            Some("item.started") | Some("item.completed") => {
                self.parse_item(&value, &mut events)
            }
            Some("turn.completed") => {
                let usage = value.get("usage").unwrap_or(&Value::Null);
                events.push(AgentEvent::Cost {
                    input_tokens: token_value(usage, "input_tokens", "inputTokens").unwrap_or(0),
                    output_tokens: token_value(usage, "output_tokens", "outputTokens")
                        .unwrap_or(0),
                    cost_usd: value.get("cost_usd").and_then(Value::as_f64),
                });
                events.push(AgentEvent::Done);
            }
            Some("turn.failed") | Some("error") => {
                let message = value
                    .pointer("/error/message")
                    .or_else(|| value.get("message"))
                    .map_or_else(|| compact_json(&value), content_text);
                events.push(AgentEvent::Text(format!("Codex error: {message}")));
                events.push(AgentEvent::Done);
            }
            _ => {}
        }
        Ok(events)
    }

    fn parse_item(&mut self, value: &Value, events: &mut Vec<AgentEvent>) {
        let Some(item) = value.get("item") else {
            return;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("agent_message") => {
                if let Some(text) = item.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    events.push(AgentEvent::Text(text.to_owned()));
                }
            }
            Some("mcp_tool_call") => {
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let name = item
                    .get("tool")
                    .and_then(Value::as_str)
                    .map(display_tool_name)
                    .unwrap_or_else(|| "tool".to_owned());
                self.tool_names.insert(id.clone(), name.clone());
                if self.announced.insert(id.clone()) {
                    events.push(AgentEvent::ToolCall {
                        name: name.clone(),
                        arguments: compact_json(
                            item.get("arguments")
                                .or_else(|| item.get("input"))
                                .unwrap_or(&Value::Null),
                        ),
                    });
                }
                if value.get("type").and_then(Value::as_str) == Some("item.completed") {
                    let result = item
                        .get("result")
                        .or_else(|| item.get("error"))
                        .or_else(|| item.get("output"))
                        .unwrap_or(&Value::Null);
                    events.push(AgentEvent::ToolResult {
                        name,
                        result: content_text(result),
                    });
                    self.tool_names.remove(&id);
                    self.announced.remove(&id);
                }
            }
            _ => {}
        }
    }
}

fn token_value(value: &Value, snake: &str, camel: &str) -> Option<u64> {
    value
        .get(snake)
        .or_else(|| value.get(camel))
        .and_then(Value::as_u64)
}

fn display_tool_name(name: &str) -> String {
    name.strip_prefix("mcp__openreel__")
        .unwrap_or(name)
        .to_owned()
}

fn content_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    if let Some(blocks) = value.as_array() {
        let text = blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            return text;
        }
    }
    compact_json(value)
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_recorded_claude_stream_shapes() {
        let fixture = include_str!("../tests/fixtures/claude-stream.jsonl");
        let mut protocol = ClaudeProtocol::default();
        let events = fixture
            .lines()
            .flat_map(|line| protocol.parse_line(line).unwrap())
            .collect::<Vec<_>>();
        assert!(events.contains(&AgentEvent::Text("I will inspect first.".to_owned())));
        assert!(events.contains(&AgentEvent::ToolCall {
            name: "get_timeline_state".to_owned(),
            arguments: "{}".to_owned(),
        }));
        assert!(events.contains(&AgentEvent::ToolResult {
            name: "get_timeline_state".to_owned(),
            result: "project fps=30/1".to_owned(),
        }));
        assert!(events.contains(&AgentEvent::Cost {
            input_tokens: 120,
            output_tokens: 34,
            cost_usd: Some(0.0042),
        }));
        assert_eq!(events.last(), Some(&AgentEvent::Done));
    }

    #[test]
    fn parses_recorded_codex_jsonl_shapes() {
        let fixture = include_str!("../tests/fixtures/codex-stream.jsonl");
        let mut protocol = CodexProtocol::default();
        let events = fixture
            .lines()
            .flat_map(|line| protocol.parse_line(line).unwrap())
            .collect::<Vec<_>>();
        assert!(events.contains(&AgentEvent::ToolCall {
            name: "split_clip".to_owned(),
            arguments: "{\"at\":30,\"clip\":1}".to_owned(),
        }));
        assert!(events.contains(&AgentEvent::ToolResult {
            name: "split_clip".to_owned(),
            result: "applied split_clip".to_owned(),
        }));
        assert!(events.contains(&AgentEvent::Text("Done.".to_owned())));
        assert_eq!(events.last(), Some(&AgentEvent::Done));
    }

    #[test]
    fn malformed_protocol_lines_report_driver_errors() {
        assert!(ClaudeProtocol::default().parse_line("not-json").is_err());
        assert!(CodexProtocol::default().parse_line("{").is_err());
    }
}
