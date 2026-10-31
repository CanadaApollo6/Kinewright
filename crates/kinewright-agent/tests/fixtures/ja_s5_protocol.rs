//! Uncertified CI bytes, consumed through the same native raw-accounting reader.
use std::io::Write;
fn main() {
    let mode = std::env::args().nth(1).unwrap();
    let mut output = std::io::stdout().lock();
    match mode.as_str() {
        "other-mcp" => {
            let _ = writeln!(
                output,
                "{{\"type\":\"item.started\",\"item\":{{\"type\":\"mcp_tool_call\",\"server\":\"ambient\",\"tool\":\"get_timeline_state\"}}}}"
            );
        }
        "other-tool" => {
            let _ = writeln!(
                output,
                "{{\"type\":\"item.started\",\"item\":{{\"type\":\"mcp_tool_call\",\"server\":\"kinewright\",\"tool\":\"bash\"}}}}"
            );
        }
        "native-tool" => {
            let _ = writeln!(
                output,
                "{{\"type\":\"item.started\",\"item\":{{\"type\":\"command_execution\"}}}}"
            );
        }
        "malformed" => {
            let _ = writeln!(output, "not-json");
        }
        "oversized" => {
            let _ = output.write_all(&vec![b'x'; 65538]);
        }
        "duplicate-cache" => {
            for _ in 0..2 {
                let _ = writeln!(
                    output,
                    "{{\"type\":\"turn.completed\",\"id\":\"same\",\"usage\":{{\"input_tokens\":20,\"cached_input_tokens\":10,\"output_tokens\":5}}}}"
                );
            }
        }
        "missing" => {
            let _ = writeln!(output, "{{\"type\":\"turn.completed\"}}");
        }
        "network-abort" => {
            let _ = writeln!(
                output,
                "{{\"type\":\"error\",\"message\":\"Reconnecting... 2/5\"}}"
            );
            let _ = writeln!(
                output,
                "{{\"type\":\"turn.failed\",\"error\":{{\"message\":\"network transport failed\"}}}}"
            );
        }
        "auth" => {
            let _ = writeln!(
                output,
                "{{\"type\":\"turn.failed\",\"error\":{{\"message\":\"authentication rejected\"}}}}"
            );
        }
        _ => {
            let _ = writeln!(
                output,
                "{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"fixture\"}}}}"
            );
        }
    }
}
