//! Audited ownership transfer in the separately exec'd supervisor.
use std::{io, os::unix::net::UnixStream};
pub fn input() -> io::Result<(u32, u64, UnixStream)> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err(io::Error::other("supervisor bootstrap arguments"));
    }
    let app = args[0].parse::<u32>().map_err(io::Error::other)?;
    let generation = args[1].parse::<u64>().map_err(io::Error::other)?;
    if app == 0 || generation == 0 {
        return Err(io::Error::other("supervisor identity"));
    }
    // SAFETY: only this exec'd supervisor takes fd 3, once. The audited wrapper
    // checks socket mode, kernel peer identity and actual parent before use.
    let socket = unsafe { kinewright_job_sys::media_scope::inherited_socket(app) }?;
    Ok((app, generation, socket))
}
