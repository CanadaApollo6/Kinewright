//! The input format shared by the target and the seed writer.
//!
//! Byte 0 selects the mode. Odd: the rest is a raw byte stream for the host's
//! frame reader. Even: the rest is a list of `[tag u8][flags u8][len u16 le]
//! [payload]` records, each wrapped in a valid header (bit 0 of `flags` sets a
//! stale request ID), so mutations reach the JSON and state validators.
//! Bits 1-2 of byte 0 choose the session's stream selection (see the target).
use kinewright_media_ipc::{Frame, HEADER_BYTES, Message, write_frame};

/// The magic and build prefix of a valid header.
pub fn prefix() -> Vec<u8> {
    let mut bytes = Vec::new();
    write_frame(
        &mut bytes,
        &Frame {
            request: 1,
            generation: 1,
            message: Message::Ready {},
        },
    )
    .expect("a valid frame");
    bytes.truncate(20);
    bytes
}

/// Wrap structured records in headers; the stream the host reader sees.
pub fn stream(prefix: &[u8], mut records: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let [tag, flags, low, high, rest @ ..] = records {
        let length = usize::from(u16::from_le_bytes([*low, *high])).min(rest.len());
        let (payload, next) = rest.split_at(length);
        let mut header = [0_u8; HEADER_BYTES];
        header[..20].copy_from_slice(prefix);
        header[20..24].copy_from_slice(&u32::from(*tag).to_le_bytes());
        header[24..28].copy_from_slice(&u32::try_from(length).unwrap_or(0).to_le_bytes());
        let request: u32 = if flags & 1 == 1 { 2 } else { 1 };
        header[28..32].copy_from_slice(&request.to_le_bytes());
        header[32..40].copy_from_slice(&1_u64.to_le_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(payload);
        records = next;
    }
    out
}
