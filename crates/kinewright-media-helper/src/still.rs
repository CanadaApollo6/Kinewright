use crate::metadata::VideoRotation;
#[derive(Clone, Copy)]
pub(crate) struct StillOrientation {
    pub rotation: VideoRotation,
}
pub(crate) fn still_bytes_orientation(bytes: &[u8]) -> Option<StillOrientation> {
    let value = jpeg_exif_orientation(bytes).or_else(|| png_exif_orientation(bytes))?;
    let (rotation, _flip_horizontal) = match value {
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
    Some(StillOrientation { rotation })
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
