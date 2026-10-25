//! Shipping host/attach/MCP-stdio entry. Credentials arrive only on the private input pipe.
use kinewright_agent::writer_host::{
    HOST_BODY_BYTES, HostBootstrap, HostClient, HostCommand, HostError, HostLaunch, WriterHost,
};
use std::io::{BufRead as _, Read as _, Write as _};
fn run() -> Result<(), HostError> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "attach".into());
    let mut input = std::io::BufReader::new(std::io::stdin().lock());
    let mut bootstrap = String::new();
    (&mut input)
        .take((HOST_BODY_BYTES + 1) as u64)
        .read_line(&mut bootstrap)?;
    if bootstrap.len() > HOST_BODY_BYTES {
        return Err(HostError::Protocol(
            "private bootstrap exceeds capacity".into(),
        ));
    }
    match mode.as_str() {
        "host" => {
            let launch: HostLaunch = serde_json::from_str(&bootstrap)?;
            let host = WriterHost::start_with_auth(launch.config, launch.auth)?;
            let mut output = std::io::stdout().lock();
            serde_json::to_writer(&mut output, host.endpoint())?;
            output.write_all(b"\n")?;
            output.flush()?;
            drop(output);
            host.wait();
        }
        "attach" | "mcp-stdio" => {
            let bootstrap: HostBootstrap = serde_json::from_str(&bootstrap)?;
            let client = HostClient::connect(bootstrap.endpoint, bootstrap.auth, bootstrap.roots)?;
            if mode == "mcp-stdio" {
                client.proxy_stdio(input, std::io::stdout().lock())?;
            } else {
                let mut output = std::io::stdout().lock();
                loop {
                    let mut line = String::new();
                    if (&mut input)
                        .take((HOST_BODY_BYTES + 1) as u64)
                        .read_line(&mut line)?
                        == 0
                    {
                        break;
                    }
                    if line.len() > HOST_BODY_BYTES {
                        return Err(HostError::Protocol("command exceeds capacity".into()));
                    }
                    let command: HostCommand = serde_json::from_str(&line)?;
                    let reply = client.request(command)?;
                    serde_json::to_writer(&mut output, &reply)?;
                    output.write_all(b"\n")?;
                    output.flush()?;
                }
            }
        }
        _ => {
            return Err(HostError::Protocol(
                "usage: kinewright host|attach|mcp-stdio (private JSON bootstrap on stdin)".into(),
            ));
        }
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
