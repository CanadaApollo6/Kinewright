//! The host's real IPC consuming path: `ProbeSession::read` (strict framing,
//! rate charge, bounded JSON) then `ProbeSession::accept` (state machine and
//! metadata validation). Any panic, out-of-bounds access or allocation over
//! the libFuzzer limit is a finding. Accepted frames must re-encode and decode.
#![no_main]
#[path = "../src/framing.rs"]
mod framing;

use kinewright_media_ipc::{Frame, ProbeSession, read_frame, write_frame};
use libfuzzer_sys::fuzz_target;
use std::time::Instant;

fn selection(mode: u8) -> [Option<u32>; 2] {
    match (mode >> 1) & 3 {
        0 => [None, None],
        1 => [Some(0), None],
        2 => [None, Some(0)],
        _ => [Some(0), Some(1)],
    }
}

fn drive(selection: [Option<u32>; 2], mut stream: &[u8]) {
    let mut session = ProbeSession::new(1, 1, selection);
    let now = Instant::now();
    // The worker reads at most these frames for one probe.
    for _ in 0..8 {
        let Ok(frame) = session.read(&mut stream, now) else {
            return;
        };
        let kind = std::mem::discriminant(&frame.message);
        let copy = Frame {
            request: frame.request,
            generation: frame.generation,
            message: frame.message.clone(),
        };
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &copy).expect("an accepted frame re-encodes");
        let decoded = read_frame(&mut encoded.as_slice()).expect("a re-encoded frame decodes");
        assert_eq!(std::mem::discriminant(&decoded.message), kind);
        match session.accept(frame) {
            Ok(Some(probe)) => {
                let metadata = probe.metadata();
                if selection != [None, None] {
                    assert_eq!([metadata.video_stream, metadata.audio_stream], selection);
                }
            }
            Ok(None) => {}
            Err(_) => return,
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let selection = selection(mode);
    if mode & 1 == 1 {
        drive(selection, rest);
    } else {
        drive(selection, &framing::stream(&framing::prefix(), rest));
    }
});
