use std::collections::{HashMap, HashSet};

use kinewright_core::{AgentError, AgentEvent};
use serde_json::Value;

#[derive(Default)]
pub(crate) struct ClaudeProtocol {
    tool_names: HashMap<String, String>,
}

impl ClaudeProtocol {
    pub(crate) fn parse_line(&mut self, line: &str) -> Result<Vec<AgentEvent>, AgentError> {
        let value: Value = serde_json::from_str(line).map_err(|error| {
            AgentError::Protocol(format!("invalid Claude stream JSON: {error}"))
        })?;
        let mut events = Vec::new();
        match value.get("type").and_then(Value::as_str) {
            Some("assistant") => self.parse_assistant(&value, &mut events),
            Some("user") => self.parse_tool_results(&value, &mut events),
            Some("result") => parse_claude_result(&value, &mut events),
            Some(other) if value.get("error").is_some() => {
                events.push(AgentEvent::Text(format!(
                    "Claude {other}: {}",
                    compact_json(value.get("error").unwrap_or(&Value::Null))
                )));
            }
            Some(_) | None => {}
        }
        Ok(events)
    }

    fn parse_assistant(&mut self, value: &Value, events: &mut Vec<AgentEvent>) {
        let Some(content) = value.pointer("/message/content").and_then(Value::as_array) else {
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
                    let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
                    let raw_name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let input = block.get("input").unwrap_or(&Value::Null);
                    let name = display_tool_call_name(raw_name, input);
                    if !id.is_empty() {
                        self.tool_names.insert(id.to_owned(), name.clone());
                    }
                    events.push(AgentEvent::ToolCall {
                        name,
                        arguments: compact_json(input),
                    });
                }
                _ => {}
            }
        }
    }

    fn parse_tool_results(&mut self, value: &Value, events: &mut Vec<AgentEvent>) {
        let Some(content) = value.pointer("/message/content").and_then(Value::as_array) else {
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
    let direct_input_tokens = token_value(usage, "input_tokens", "inputTokens")
        .or_else(|| model_usage_total(value, "inputTokens"))
        .unwrap_or(0);
    let cached_input_tokens = token_value(usage, "cache_read_input_tokens", "cacheReadInputTokens")
        .or_else(|| model_usage_total(value, "cacheReadInputTokens"));
    let cache_creation_input_tokens = token_value(
        usage,
        "cache_creation_input_tokens",
        "cacheCreationInputTokens",
    )
    .or_else(|| model_usage_total(value, "cacheCreationInputTokens"));
    let input_tokens = direct_input_tokens
        .saturating_add(cached_input_tokens.unwrap_or(0))
        .saturating_add(cache_creation_input_tokens.unwrap_or(0));
    let output_tokens = token_value(usage, "output_tokens", "outputTokens")
        .or_else(|| model_usage_total(value, "outputTokens"))
        .unwrap_or(0);
    events.push(AgentEvent::Cost {
        input_tokens,
        cached_input_tokens,
        cache_creation_input_tokens,
        output_tokens,
        reasoning_output_tokens: token_value(
            usage,
            "reasoning_output_tokens",
            "reasoningOutputTokens",
        )
        .or_else(|| model_usage_total(value, "reasoningOutputTokens")),
        cost_usd: value.get("total_cost_usd").and_then(Value::as_f64),
    });
    events.push(AgentEvent::Done);
}

fn model_usage_total(value: &Value, key: &str) -> Option<u64> {
    let usage = value
        .get("modelUsage")
        .or_else(|| value.get("model_usage"))?
        .as_object()?;
    let values = usage
        .values()
        .filter_map(|model| model.get(key).and_then(Value::as_u64))
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.into_iter().sum())
}

#[derive(Default)]
pub(crate) struct CodexProtocol {
    tool_names: HashMap<String, String>,
    announced: HashSet<String>,
}

impl CodexProtocol {
    pub(crate) fn parse_line(&mut self, line: &str) -> Result<Vec<AgentEvent>, AgentError> {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| AgentError::Protocol(format!("invalid Codex JSONL: {error}")))?;
        let mut events = Vec::new();
        match value.get("type").and_then(Value::as_str) {
            Some("item.started" | "item.completed") => self.parse_item(&value, &mut events),
            Some("turn.completed") => {
                let usage = value.get("usage").unwrap_or(&Value::Null);
                events.push(AgentEvent::Cost {
                    input_tokens: token_value(usage, "input_tokens", "inputTokens").unwrap_or(0),
                    cached_input_tokens: token_value(
                        usage,
                        "cached_input_tokens",
                        "cachedInputTokens",
                    )
                    .or_else(|| nested_token_value(usage, "input_tokens_details", "cached_tokens"))
                    .or_else(|| nested_token_value(usage, "inputTokensDetails", "cachedTokens")),
                    cache_creation_input_tokens: token_value(
                        usage,
                        "cache_write_input_tokens",
                        "cacheWriteInputTokens",
                    ),
                    output_tokens: token_value(usage, "output_tokens", "outputTokens").unwrap_or(0),
                    reasoning_output_tokens: token_value(
                        usage,
                        "reasoning_output_tokens",
                        "reasoningOutputTokens",
                    )
                    .or_else(|| {
                        nested_token_value(usage, "output_tokens_details", "reasoning_tokens")
                    })
                    .or_else(|| {
                        nested_token_value(usage, "outputTokensDetails", "reasoningTokens")
                    }),
                    cost_usd: value
                        .get("cost_usd")
                        .or_else(|| usage.get("cost_usd"))
                        .and_then(Value::as_f64),
                });
                events.push(AgentEvent::Done);
            }
            Some("turn.failed" | "error") => {
                let message = value
                    .pointer("/error/message")
                    .or_else(|| value.get("message"))
                    .map_or_else(|| compact_json(&value), content_text);
                events.push(AgentEvent::Error(format!("Codex error: {message}")));
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
                let arguments = item
                    .get("arguments")
                    .or_else(|| item.get("input"))
                    .unwrap_or(&Value::Null);
                let name = item.get("tool").and_then(Value::as_str).map_or_else(
                    || "tool".to_owned(),
                    |name| display_tool_call_name(name, arguments),
                );
                self.tool_names.insert(id.clone(), name.clone());
                if self.announced.insert(id.clone()) {
                    events.push(AgentEvent::ToolCall {
                        name: name.clone(),
                        arguments: compact_json(arguments),
                    });
                }
                if value.get("type").and_then(Value::as_str) == Some("item.completed") {
                    events.push(AgentEvent::ToolResult {
                        name,
                        result: codex_tool_result(item),
                    });
                    self.tool_names.remove(&id);
                    self.announced.remove(&id);
                }
            }
            Some("error")
                if value.get("type").and_then(Value::as_str) == Some("item.completed") =>
            {
                let message = item
                    .get("message")
                    .map_or_else(|| compact_json(item), content_text);
                events.push(AgentEvent::Error(format!("Codex error: {message}")));
            }
            _ => {}
        }
    }
}

fn codex_tool_result(item: &Value) -> String {
    if let Some(error) = item.get("error").filter(|error| !error.is_null()) {
        return content_text(error);
    }
    let result = item
        .get("result")
        .or_else(|| item.get("output"))
        .unwrap_or(&Value::Null);
    if let Some(content) = result.get("content") {
        let text = content_text(content);
        if text != "null" && text != "[]" {
            return text;
        }
    }
    if let Some(structured) = result
        .get("structured_content")
        .filter(|structured| !structured.is_null())
    {
        return compact_json(structured);
    }
    content_text(result)
}

fn token_value(value: &Value, snake: &str, camel: &str) -> Option<u64> {
    value
        .get(snake)
        .or_else(|| value.get(camel))
        .and_then(Value::as_u64)
}

fn nested_token_value(value: &Value, object: &str, field: &str) -> Option<u64> {
    value.get(object)?.get(field)?.as_u64()
}

fn display_tool_name(name: &str) -> String {
    name.strip_prefix("mcp__kinewright__")
        .unwrap_or(name)
        .to_owned()
}

fn display_tool_call_name(name: &str, arguments: &Value) -> String {
    let display = display_tool_name(name);
    if display == "invoke_capability" {
        arguments
            .get("name")
            .and_then(Value::as_str)
            .map_or(display, str::to_owned)
    } else {
        display
    }
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
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            output_tokens: 34,
            reasoning_output_tokens: None,
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
            name: "probe_echo".to_owned(),
            arguments: "{\"text\":\"mcp-ok\"}".to_owned(),
        }));
        assert!(events.contains(&AgentEvent::ToolResult {
            name: "probe_echo".to_owned(),
            result: "KINEWRIGHT_PROBE:mcp-ok".to_owned(),
        }));
        assert!(events.contains(&AgentEvent::Text(
            "MCP succeeded; no built-in write tool was available.".to_owned()
        )));
        assert!(events.contains(&AgentEvent::Cost {
            input_tokens: 3,
            cached_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
            output_tokens: 3,
            reasoning_output_tokens: Some(0),
            cost_usd: None,
        }));
        assert_eq!(events.last(), Some(&AgentEvent::Done));
    }

    #[test]
    fn malformed_protocol_lines_report_driver_errors() {
        assert!(ClaudeProtocol::default().parse_line("not-json").is_err());
        assert!(CodexProtocol::default().parse_line("{").is_err());
    }

    #[test]
    fn claude_usage_normalizes_cache_reads_and_writes_into_total_input() {
        let events = ClaudeProtocol::default()
            .parse_line(
                r#"{"type":"result","is_error":false,"modelUsage":{"claude":{"inputTokens":20,"cacheReadInputTokens":80,"cacheCreationInputTokens":10,"outputTokens":12}},"total_cost_usd":0.01}"#,
            )
            .unwrap();
        assert_eq!(
            events[0],
            AgentEvent::Cost {
                input_tokens: 110,
                cached_input_tokens: Some(80),
                cache_creation_input_tokens: Some(10),
                output_tokens: 12,
                reasoning_output_tokens: None,
                cost_usd: Some(0.01),
            }
        );
    }

    #[test]
    fn compact_dispatch_reports_the_underlying_capability_name() {
        let events = CodexProtocol::default()
            .parse_line(
                r#"{"type":"item.started","item":{"id":"tool-1","type":"mcp_tool_call","tool":"mcp__kinewright__invoke_capability","arguments":{"name":"get_timeline_transcript","arguments":{}}}}"#,
            )
            .unwrap();
        assert_eq!(
            events,
            vec![AgentEvent::ToolCall {
                name: "get_timeline_transcript".to_owned(),
                arguments: r#"{"arguments":{},"name":"get_timeline_transcript"}"#.to_owned(),
            }]
        );
    }
}
