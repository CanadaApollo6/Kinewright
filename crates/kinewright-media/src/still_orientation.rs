//! MO1 R7: still-image display orientation (EXIF).
//!
//! `FFmpeg` ignores EXIF orientation (verified: an orientation-6 JPEG decodes
//! unrotated with no rotation metadata), so stills carry it here. JPEG APP1
//! EXIF and PNG eXIf share one TIFF IFD0 parser; values 1–8 map to a
//! right-angle rotation plus an optional pre-rotation horizontal flip.
//!
//! Anything unrecognized — missing, truncated, out of range — is `None`: the
//! stored pixels display as-is, exactly like an untagged still. Orientation
//! is advisory; it never fails a probe or a decode (a genuinely unreadable
//! file fails those with their own errors).

use std::path::Path;

use crate::decode::VideoRotation;

// Bytes read from disk by `still_file_orientation` on this thread
// (tests only): the G4 pin that video opens never pay for a whole-file
// orientation scan. Thread-local so parallel tests never share counts —
// the decoder reads it synchronously on the opener.
#[cfg(test)]
std::thread_local! {
    static ORIENTATION_BYTES_READ: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_orientation_bytes_read() {
    ORIENTATION_BYTES_READ.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn orientation_bytes_read() -> u64 {
    ORIENTATION_BYTES_READ.with(std::cell::Cell::get)
}

#[cfg(test)]
fn count_orientation_bytes_read(bytes: u64) {
    ORIENTATION_BYTES_READ.with(|count| count.set(count.get() + bytes));
}

/// A still's display transform: flip horizontally in stored-pixel space,
/// then rotate right-angle. (EXIF 2/4/5/7 are the mirrored set; 5 and 7 are
/// the transpose pair — flip-then-rotate composes them exactly.)
pub(crate) struct StillOrientation {
    pub(crate) rotation: VideoRotation,
    pub(crate) flip_horizontal: bool,
}

/// The orientation header window: EXIF lives in the file head, so the
/// walk never reads past 64 KB (G4 — video opens once paid a whole-file
/// read here). Orientation past the window is missed, which is safe:
/// orientation is advisory and the pixels display as-is.
const ORIENTATION_HEADER_BYTES: u64 = 65_536;

/// Read a still file's EXIF orientation, if it carries a valid one.
///
/// Returns `None` for non-stills, for stills without orientation, and for
/// corrupt headers — never an error. Reads the 8-byte signature first so
/// containers fail before any window read, then at most
/// [`ORIENTATION_HEADER_BYTES`] total.
pub(crate) fn still_file_orientation(path: &Path) -> Option<StillOrientation> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut signature = [0_u8; 8];
    let signature_len = file.read(&mut signature).ok()? as u64;
    #[cfg(test)]
    count_orientation_bytes_read(signature_len);
    if signature_len < 8 {
        return None;
    }
    let is_jpeg = signature[0] == 0xFF && signature[1] == 0xD8;
    let is_png = signature == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if !is_jpeg && !is_png {
        return None;
    }
    let mut bytes = signature.to_vec();
    file.take(ORIENTATION_HEADER_BYTES - signature_len)
        .read_to_end(&mut bytes)
        .ok()?;
    #[cfg(test)]
    count_orientation_bytes_read(bytes.len() as u64 - signature_len);
    let value = jpeg_exif_orientation(&bytes).or_else(|| png_exif_orientation(&bytes))?;
    let (rotation, flip_horizontal) = match value {
        1 => (VideoRotation::None, false),
        2 => (VideoRotation::None, true),
        3 => (VideoRotation::HalfTurn, false),
        4 => (VideoRotation::HalfTurn, true),
        // G2: 5 is the transpose (flip + 90 CCW) and 7 the transverse
        // (flip + 90 CW) -- verified against the EXIF spec display
        // mapping and ImageMagick -auto-orient.
        5 => (VideoRotation::Clockwise270, true),
        6 => (VideoRotation::Clockwise90, false),
        7 => (VideoRotation::Clockwise90, true),
        8 => (VideoRotation::Clockwise270, false),
        _ => return None,
    };
    Some(StillOrientation {
        rotation,
        flip_horizontal,
    })
}

/// Scan JPEG segments for an APP1 EXIF orientation value.
fn jpeg_exif_orientation(bytes: &[u8]) -> Option<u8> {
    if bytes.len() < 2 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut offset = 2_usize;
    while offset + 4 <= bytes.len() {
        if bytes[offset] != 0xFF {
            return None;
        }
        // Skip fill bytes; the marker is the first non-FF byte.
        let mut marker_at = offset + 1;
        while marker_at < bytes.len() && bytes[marker_at] == 0xFF {
            marker_at += 1;
        }
        if marker_at >= bytes.len() {
            return None;
        }
        let marker = bytes[marker_at];
        // Standalone markers carry no length; SOS starts the scan data.
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            offset = marker_at + 1;
            continue;
        }
        if marker == 0xDA {
            return None;
        }
        if marker_at + 3 > bytes.len() {
            return None;
        }
        let length = usize::from(u16::from_be_bytes([
            bytes[marker_at + 1],
            bytes[marker_at + 2],
        ]));
        if length < 2 || marker_at + 1 + length > bytes.len() {
            return None;
        }
        let payload = &bytes[marker_at + 3..marker_at + 1 + length];
        if marker == 0xE1
            && payload.len() > 6
            && payload[..6] == *b"Exif\0\0"
            && let Some(value) = exif_orientation_from_tiff(&payload[6..])
        {
            return Some(value);
        }
        offset = marker_at + 1 + length;
    }
    None
}

/// Walk PNG chunks for an eXIf orientation value.
fn png_exif_orientation(bytes: &[u8]) -> Option<u8> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 8 || bytes[..8] != SIGNATURE {
        return None;
    }
    let mut offset = 8_usize;
    while offset + 12 <= bytes.len() {
        let length = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]);
        let length = usize::try_from(length).unwrap_or(usize::MAX);
        let data_end = offset.saturating_add(8).saturating_add(length);
        // Chunk layout is length + type + data + CRC.
        if data_end.saturating_add(4) > bytes.len() {
            return None;
        }
        let chunk_type = &bytes[offset + 4..offset + 8];
        if chunk_type == b"eXIf"
            && let Some(value) = exif_orientation_from_tiff(&bytes[offset + 8..data_end])
        {
            return Some(value);
        }
        if chunk_type == b"IEND" {
            return None;
        }
        offset = data_end.saturating_add(4);
    }
    None
}

/// Read tag 0x0112 (Orientation) from a TIFF header's first IFD.
fn exif_orientation_from_tiff(tiff: &[u8]) -> Option<u8> {
    if tiff.len() < 8 {
        return None;
    }
    let little = match &tiff[..4] {
        [b'I', b'I', 0x2A, 0x00] => true,
        [b'M', b'M', 0x00, 0x2A] => false,
        _ => return None,
    };
    let read_u16 = |at: usize| {
        let pair: [u8; 2] = tiff.get(at..at + 2)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        })
    };
    let read_u32 = |at: usize| {
        let quad: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(quad)
        } else {
            u32::from_be_bytes(quad)
        })
    };
    let ifd = usize::try_from(read_u32(4)?).ok()?;
    let entries = usize::from(read_u16(ifd.checked_add(0)?)?);
    for index in 0..entries {
        let entry = ifd.checked_add(2)?.checked_add(index.checked_mul(12)?)?;
        if entry.checked_add(12)? > tiff.len() {
            return None;
        }
        if read_u16(entry)? != 0x0112 {
            continue;
        }
        // SHORT, count 1: the value sits inline in the first two value bytes.
        if read_u16(entry + 2)? != 3 || read_u32(entry + 4)? != 1 {
            return None;
        }
        let value = read_u16(entry + 8)?;
        return u8::try_from(value)
            .ok()
            .filter(|value| (1..=8).contains(value));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal TIFF header with IFD0 carrying the given (tag, type, count,
    /// raw value bytes), little-endian unless `big` is set.
    fn tiff(tag: u16, field_type: u16, count: u32, value: [u8; 4], big: bool) -> Vec<u8> {
        let mut header = if big {
            vec![b'M', b'M', 0x00, 0x2A, 0x00, 0x00, 0x00, 0x08]
        } else {
            vec![b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00]
        };
        let (u16, u32) = if big {
            (
                u16::to_be_bytes as fn(u16) -> [u8; 2],
                u32::to_be_bytes as fn(u32) -> [u8; 4],
            )
        } else {
            (
                u16::to_le_bytes as fn(u16) -> [u8; 2],
                u32::to_le_bytes as fn(u32) -> [u8; 4],
            )
        };
        header.extend_from_slice(&u16(1));
        header.extend_from_slice(&u16(tag));
        header.extend_from_slice(&u16(field_type));
        header.extend_from_slice(&u32(count));
        header.extend_from_slice(&value);
        header.extend_from_slice(&u32(0));
        header
    }

    #[test]
    fn tiff_orientation_reads_both_byte_orders() {
        for big in [false, true] {
            for value in 1..=8_u8 {
                let inline = if big {
                    [0x00, value, 0x00, 0x00]
                } else {
                    [value, 0x00, 0x00, 0x00]
                };
                assert_eq!(
                    exif_orientation_from_tiff(&tiff(0x0112, 3, 1, inline, big)),
                    Some(value),
                    "byte order big={big}, value={value}"
                );
            }
        }
    }

    #[test]
    fn tiff_orientation_rejects_garbage() {
        // Bad headers.
        assert_eq!(exif_orientation_from_tiff(&[]), None);
        assert_eq!(exif_orientation_from_tiff(&[0x49; 7]), None);
        assert_eq!(
            exif_orientation_from_tiff(&[b'X', b'X', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00]),
            None
        );
        // Wrong tag, wrong type, wrong count, out-of-range value.
        assert_eq!(
            exif_orientation_from_tiff(&tiff(0x0100, 3, 1, [6, 0, 0, 0], false)),
            None
        );
        assert_eq!(
            exif_orientation_from_tiff(&tiff(0x0112, 4, 1, [6, 0, 0, 0], false)),
            None
        );
        assert_eq!(
            exif_orientation_from_tiff(&tiff(0x0112, 3, 2, [6, 0, 0, 0], false)),
            None
        );
        assert_eq!(
            exif_orientation_from_tiff(&tiff(0x0112, 3, 1, [9, 0, 0, 0], false)),
            None
        );
        // Truncated IFD entry (the 12-byte entry at offset 10 is cut).
        let mut header = tiff(0x0112, 3, 1, [6, 0, 0, 0], false);
        header.truncate(21);
        assert_eq!(exif_orientation_from_tiff(&header), None);
    }

    #[test]
    fn jpeg_scan_finds_app1_before_sos_and_ignores_the_rest() {
        let tiff = tiff(0x0112, 3, 1, [6, 0, 0, 0], false);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let segment = |marker: u8, payload: &[u8]| {
            let mut segment = vec![0xFF, marker];
            let length = u16::try_from(payload.len() + 2).unwrap().to_be_bytes();
            segment.extend_from_slice(&length);
            segment.extend_from_slice(payload);
            segment
        };
        // APP0 (JFIF) then APP1: the scan skips to the EXIF segment.
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend_from_slice(&segment(0xE0, b"JFIF\0junk"));
        jpeg.extend_from_slice(&segment(0xE1, &app1));
        assert_eq!(jpeg_exif_orientation(&jpeg), Some(6));

        // EXIF after SOS is image data, not a header.
        let mut sos = vec![0xFF, 0xD8];
        sos.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06]);
        sos.extend_from_slice(&segment(0xE1, &app1));
        assert_eq!(jpeg_exif_orientation(&sos), None);

        // Not a JPEG, truncated segment, corrupt TIFF inside APP1.
        assert_eq!(jpeg_exif_orientation(&[]), None);
        assert_eq!(jpeg_exif_orientation(&[0xFF, 0xD8, 0xFF]), None);
        let mut truncated = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x20];
        truncated.extend_from_slice(b"Exif\0\0short");
        assert_eq!(jpeg_exif_orientation(&truncated), None);
        let mut corrupt = vec![0xFF, 0xD8];
        corrupt.extend_from_slice(&segment(0xE1, b"Exif\0\0bogus"));
        assert_eq!(jpeg_exif_orientation(&corrupt), None);
    }

    #[test]
    fn png_scan_finds_exif_and_stops_at_iend() {
        let tiff = tiff(0x0112, 3, 1, [6, 0, 0, 0], false);
        let chunk = |kind: &[u8; 4], data: &[u8]| {
            let mut chunk = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
            chunk.extend_from_slice(kind);
            chunk.extend_from_slice(data);
            // The CRC is unchecked — orientation never gates on it.
            chunk.extend_from_slice(&[0, 0, 0, 0]);
            chunk
        };
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&chunk(b"IHDR", &[0; 13]));
        png.extend_from_slice(&chunk(b"eXIf", &tiff));
        png.extend_from_slice(&chunk(b"IEND", &[]));
        assert_eq!(png_exif_orientation(&png), Some(6));

        let mut bare = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bare.extend_from_slice(&chunk(b"IHDR", &[0; 13]));
        bare.extend_from_slice(&chunk(b"IEND", &[]));
        assert_eq!(png_exif_orientation(&bare), None);
        assert_eq!(png_exif_orientation(&[]), None);
        assert_eq!(png_exif_orientation(b"not a png file...."), None);
    }

    #[test]
    fn all_eight_orientations_map() {
        let rotations = [
            (VideoRotation::None, false),
            (VideoRotation::None, true),
            (VideoRotation::HalfTurn, false),
            (VideoRotation::HalfTurn, true),
            (VideoRotation::Clockwise270, true),
            (VideoRotation::Clockwise90, false),
            (VideoRotation::Clockwise90, true),
            (VideoRotation::Clockwise270, false),
        ];
        for (index, (rotation, flip)) in rotations.into_iter().enumerate() {
            let value = u8::try_from(index + 1).unwrap();
            let dir = crate::test_support::TempDirectory::new("mo1-orientation");
            let path = dir.path("oriented.jpg");
            let tiff = tiff(0x0112, 3, 1, [value, 0, 0, 0], false);
            let mut app1 = b"Exif\0\0".to_vec();
            app1.extend_from_slice(&tiff);
            let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
            let length = u16::try_from(app1.len() + 2).unwrap().to_be_bytes();
            jpeg.extend_from_slice(&length);
            jpeg.extend_from_slice(&app1);
            std::fs::write(&path, jpeg).unwrap();
            let orientation = still_file_orientation(&path).expect("orientation should parse");
            assert_eq!(orientation.rotation, rotation, "value {value}");
            assert_eq!(orientation.flip_horizontal, flip, "value {value}");
        }
        let missing = crate::test_support::TempDirectory::new("mo1-orientation-missing");
        assert!(
            still_file_orientation(&missing.path("nope.jpg")).is_none(),
            "an unreadable file carries no orientation"
        );
    }

    /// MO1 G4: opening an MJPEG video reads under 64 KB for the
    /// orientation probe — never the whole file (the 62 MB repro).
    #[test]
    fn mjpeg_video_open_reads_less_than_64k_for_orientation() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize");
        let media = crate::test_support::GeneratedMedia::ffmpeg(
            "mo1-g4-mjpeg-video",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x360:rate=25:duration=8",
                "-c:v",
                "mjpeg",
                "-q:v",
                "2",
                "-an",
            ],
            "avi",
        );
        let size = std::fs::metadata(media.path())
            .expect("video should exist")
            .len();
        assert!(
            size > 65_536,
            "the repro file must exceed the bound to be meaningful, got {size}"
        );
        reset_orientation_bytes_read();
        let decoder = crate::decode::VideoDecoder::open(
            media.path(),
            kinewright_core::Rational::new(25, 1).unwrap(),
        )
        .expect("the MJPEG video should open");
        drop(decoder);
        let read = orientation_bytes_read();
        assert!(
            read < 65_536,
            "opening a {size}-byte MJPEG video read {read} bytes for orientation"
        );

        // With an audio stream the open path skips the probe entirely.
        let sounded = crate::test_support::GeneratedMedia::ffmpeg(
            "mo1-g4-mjpeg-sounded",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=25:duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-c:v",
                "mjpeg",
                "-q:v",
                "2",
                "-c:a",
                "pcm_s16le",
                "-shortest",
            ],
            "avi",
        );
        reset_orientation_bytes_read();
        drop(
            crate::decode::VideoDecoder::open(
                sounded.path(),
                kinewright_core::Rational::new(25, 1).unwrap(),
            )
            .expect("the sounded MJPEG video should open"),
        );
        assert_eq!(
            orientation_bytes_read(),
            0,
            "an audio-carrying MJPEG video must skip the orientation probe"
        );
    }

    /// MO1 G4: the orientation walk is header-bounded — EXIF past the
    /// 64 KB header window is missed (orientation is advisory), while a
    /// normal still parses within the window.
    #[test]
    fn still_orientation_read_is_header_bounded() {
        let dir = crate::test_support::TempDirectory::new("mo1-g4-header-cap");
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff(0x0112, 3, 1, [6, 0, 0, 0], false));
        // SOI + two 40 KB filler segments push the APP1 EXIF past 64 KB.
        let mut jpeg = vec![0xFF, 0xD8];
        for _ in 0..2 {
            let filler = vec![0u8; 40_000];
            jpeg.extend_from_slice(&[0xFF, 0xE0]);
            jpeg.extend_from_slice(&(u16::try_from(filler.len()).unwrap() + 2).to_be_bytes());
            jpeg.extend_from_slice(&filler);
        }
        jpeg.extend_from_slice(&[0xFF, 0xE1]);
        jpeg.extend_from_slice(&(u16::try_from(app1.len()).unwrap() + 2).to_be_bytes());
        jpeg.extend_from_slice(&app1);
        let path = dir.path("padded.jpg");
        std::fs::write(&path, &jpeg).unwrap();

        reset_orientation_bytes_read();
        assert!(
            still_file_orientation(&path).is_none(),
            "EXIF past the header window must be missed, not whole-file scanned"
        );
        assert!(
            orientation_bytes_read() <= 65_536,
            "the header walk must stay bounded, read {}",
            orientation_bytes_read()
        );

        // A normal still still parses, within the window.
        let plain = dir.path("plain.jpg");
        let mut normal = vec![0xFF, 0xD8, 0xFF, 0xE1];
        normal.extend_from_slice(&(u16::try_from(app1.len()).unwrap() + 2).to_be_bytes());
        normal.extend_from_slice(&app1);
        std::fs::write(&plain, &normal).unwrap();
        reset_orientation_bytes_read();
        assert!(
            still_file_orientation(&plain).is_some(),
            "in-window EXIF must parse"
        );
        assert!(
            orientation_bytes_read() <= 65_536,
            "a normal still must read within the window"
        );
    }
}
