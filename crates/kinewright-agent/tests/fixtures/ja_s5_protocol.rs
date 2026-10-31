//! Uncertified CI bytes, consumed through the same native raw-accounting reader.
use std::io::Write;
fn main() {
    let mode = std::env::args().nth(1).unwrap();
    let mut output = std::io::stdout().lock();
    match mode.as_str() {
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
