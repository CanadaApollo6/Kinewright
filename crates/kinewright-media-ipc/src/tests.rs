use super::*;

fn encoded(message: Message) -> Vec<u8> {
    let mut bytes = vec![];
    write_frame(
        &mut bytes,
        &Frame {
            request: 1,
            generation: 2,
            message,
        },
    )
    .unwrap();
    bytes
}
fn payload(bytes: &[u8]) -> Vec<u8> {
    let mut frame = encoded(Message::Ready {});
    frame.truncate(HEADER_BYTES);
    frame[24..28].copy_from_slice(&u32::try_from(bytes.len()).unwrap().to_le_bytes());
    frame.extend_from_slice(bytes);
    frame
}
#[test]
fn framing_roundtrip_and_all_truncations() {
    let bytes = encoded(Message::Ready {});
    assert!(matches!(
        read_frame(&mut &bytes[..]).unwrap().message,
        Message::Ready {}
    ));
    for length in 0..bytes.len() {
        assert!(read_frame(&mut &bytes[..length]).is_err());
    }
    for index in 0..24 {
        let mut corrupted = bytes.clone();
        corrupted[index] ^= 128;
        assert!(read_frame(&mut &corrupted[..]).is_err());
    }
    let mut huge = bytes;
    huge[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(read_frame(&mut &huge[..]).is_err());
}
#[test]
fn strict_json_including_nested_and_escaped_duplicate_keys() {
    for json in [
        r#"{"type":"Ready","extra":1}"#,
        r#"{"type":"Ready","type":"Ready"}"#,
        r#"{"type":"Ready","nested":{"a":1,"\u0061":2}}"#,
        r#"{"type":"Ready"} {}"#,
        r#"{"type":"Ready","x":[[[[[[[[[0]]]]]]]]]}"#,
    ] {
        assert!(
            read_frame(&mut &payload(json.as_bytes())[..]).is_err(),
            "{json}"
        );
    }
    assert!(json::check(br#"{"a":{"b":1,"b":2}}"#).is_err());
    assert!(json::check(br#"{"a":{"b":1},"b":2}"#).is_ok());
    assert!(read_frame(&mut &payload(&[255])[..]).is_err());
}
#[test]
fn state_rejects_stale_out_of_order_and_flooded_traffic() {
    let frame = || read_frame(&mut &encoded(Message::Ready {})[..]).unwrap();
    let mut session = ProbeSession::new(1, 2);
    assert!(session.accept(frame()).unwrap().is_none());
    assert!(session.accept(frame()).is_err());
    assert!(ProbeSession::new(1, 3).accept(frame()).is_err());
    assert!(
        ProbeSession::new(1, 2)
            .accept(Frame {
                request: 1,
                generation: 2,
                message: Message::Complete {}
            })
            .is_err()
    );
    let now = std::time::Instant::now();
    let mut session = ProbeSession::new(1, 2);
    for _ in 0..256 {
        session.charge(40, now).unwrap();
    }
    assert!(session.charge(40, now).is_err());
}
#[test]
fn bounded_arbitrary_bytes_never_panic() {
    let mut state = 42_u64;
    for size in 0..1024 {
        let mut bytes = Vec::with_capacity(size);
        for _ in 0..size {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            bytes.push(state.to_le_bytes()[0]);
        }
        let _ = read_frame(&mut &bytes[..]);
        let _ = json::check(&bytes);
    }
}

fn probe() -> ProbeResult {
    ProbeResult {
        streams: vec![Stream {
            index: 0,
            kind: StreamKind::Video,
            time_base: [1, 30],
            timestamp_origin: 0,
            duration: 30,
            average_frame_rate: [30, 1],
        }],
        duration_microseconds: 1_000_000,
        video_stream: Some(0),
        audio_stream: None,
        duration_frames: 30,
        fps: [30, 1],
        kind: Kind::Video,
        resolution: Some([1920, 1080]),
        color: Color {
            primaries: kinewright_core::ColorPrimaries::Unknown,
            transfer: kinewright_core::ColorTransfer::Unknown,
            matrix: kinewright_core::ColorMatrix::Unknown,
            range: kinewright_core::ColorRange::Unknown,
            white_point: kinewright_core::ColorWhitePoint::Unknown,
            bit_depth: kinewright_core::ColorBitDepth::Unknown,
            confidence_basis_points: 0,
            provenance: kinewright_core::ColorProvenance::Unknown,
        },
    }
}
#[test]
fn metadata_bounds_and_actual_domain_consumer() {
    let value = ValidatedProbe::new(probe()).unwrap().into_metadata();
    let rate = kinewright_core::Rational::new(value.fps[0], value.fps[1]).unwrap();
    assert_eq!(rate.numerator(), 30);
    let mut cases = vec![];
    let mut bad = probe();
    bad.fps = [0, 1];
    cases.push(bad);
    let mut bad = probe();
    bad.fps = [u32::MAX, 1];
    cases.push(bad);
    let mut bad = probe();
    bad.fps = [60, 2];
    cases.push(bad);
    let mut bad = probe();
    bad.video_stream = Some(16);
    cases.push(bad);
    let mut bad = probe();
    bad.streams[0].time_base = [1, 0];
    cases.push(bad);
    let mut bad = probe();
    bad.streams[0].timestamp_origin = i64::MAX;
    cases.push(bad);
    let mut bad = probe();
    bad.duration_frames = i64::MAX;
    cases.push(bad);
    let mut bad = probe();
    bad.resolution = Some([8192, 8192]);
    cases.push(bad);
    let mut bad = probe();
    bad.color.confidence_basis_points = 10001;
    cases.push(bad);
    let mut bad = probe();
    bad.color.primaries = kinewright_core::ColorPrimaries::Other("forged".into());
    cases.push(bad);
    for case in cases {
        assert!(ValidatedProbe::new(case).is_err());
    }
}
