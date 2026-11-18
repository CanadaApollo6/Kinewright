use super::*;
use kinewright_core::{
    ColorBitDepth, ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange, ColorTransfer,
    ColorWhitePoint,
};
use proptest::prelude::*;

fn probe_kind(kind: Kind) -> ProbeResult {
    let mut result = super::probe();
    result.kind = kind;
    if kind == Kind::Image {
        result.duration_frames = 1;
        result.duration_microseconds = 33_334;
    }
    if kind == Kind::Audio {
        result.streams.clear();
        result.video_stream = None;
        result.resolution = None;
    }
    if matches!(kind, Kind::Audio | Kind::AudioVideo) {
        let index = u32::from(result.video_stream.is_some());
        result.audio_stream = Some(index);
        result.streams.push(Stream {
            index,
            kind: StreamKind::Audio,
            time_base: [1, 48_000],
            timestamp_origin: 0,
            duration: 48_000,
            average_frame_rate: [0, 1],
        });
    }
    result
}

#[test]
fn still_probe_refuses_unbounded_microsecond_duration() {
    let mut result = probe_kind(Kind::Image);
    assert!(ValidatedProbe::new(result.clone()).is_ok());
    result.duration_microseconds = i64::MIN;
    assert!(matches!(
        ValidatedProbe::new(result),
        Err(MediaSandboxError::ProtocolViolation)
    ));
}

#[test]
fn audio_probe_refuses_invalid_average_frame_rate() {
    let mut result = probe_kind(Kind::Audio);
    assert!(ValidatedProbe::new(result.clone()).is_ok());
    result.streams[0].average_frame_rate = [-1, 0];
    assert!(matches!(
        ValidatedProbe::new(result),
        Err(MediaSandboxError::ProtocolViolation)
    ));
}

fn assert_fps([n, d]: [u32; 2]) {
    let reduced = kinewright_core::Rational::new(n, d).unwrap();
    assert_eq!([reduced.numerator(), reduced.denominator()], [n, d]);
    assert!(f64::from(n) / f64::from(d) <= 4096.0);
}

// Mutate one field at a time so unrelated checks cannot mask missing guards.
// Exercise a valid reply of each kind on every generated case as a control.
fn assert_table(result: &ProbeResult) {
    const MAX_US: i64 = 604_800_000_000;
    assert!((1..=16).contains(&result.streams.len()));
    for (index, stream) in result.streams.iter().enumerate() {
        assert_eq!(usize::try_from(stream.index).unwrap(), index);
        let [n, d] = stream.time_base.map(i128::from);
        assert!(n > 0 && d > 0);
        assert!(stream.duration >= 0);
        assert!(
            stream
                .timestamp_origin
                .checked_add(stream.duration)
                .is_some()
        );
        for ticks in [stream.timestamp_origin, stream.duration] {
            assert!(i128::from(ticks).abs() * n * 1_000_000 <= i128::from(MAX_US) * d);
        }
        match (stream.kind, result.kind, stream.average_frame_rate) {
            (StreamKind::Audio, _, rate) => assert_eq!(rate, [0, 1]),
            (StreamKind::Video, Kind::Image, [0, 1]) => {}
            (StreamKind::Video, _, rate) => {
                assert_fps(rate.map(|component| u32::try_from(component).unwrap()));
            }
        }
    }
    for (index, kind) in [
        (result.video_stream, StreamKind::Video),
        (result.audio_stream, StreamKind::Audio),
    ] {
        if let Some(index) = index {
            assert_eq!(result.streams[usize::try_from(index).unwrap()].kind, kind);
        }
    }
    assert_eq!(result.video_stream.is_some(), result.kind != Kind::Audio);
    assert_eq!(
        result.audio_stream.is_some(),
        matches!(result.kind, Kind::Audio | Kind::AudioVideo)
    );
    assert_eq!(result.resolution.is_some(), result.video_stream.is_some());
    if let Some([width, height]) = result.resolution {
        assert!((1..=8192).contains(&width) && (1..=8192).contains(&height));
        assert!(u64::from(width) * u64::from(height) <= (128 << 20) / 8);
    }
    assert_fps(result.fps);
    assert!((1..=MAX_US).contains(&result.duration_microseconds));
    let frames = i128::from(result.duration_frames);
    let [n, d] = result.fps.map(i128::from);
    assert!(frames > 0 && frames * d <= 604_800 * n);
    if result.kind == Kind::Image {
        assert_eq!(frames, 1);
    }
    assert!(result.color.confidence_basis_points <= 10_000);
    if let kinewright_core::ColorBitDepth::Integer(bits) = result.color.bit_depth {
        assert!((1..=32).contains(&bits));
    }
    if let ColorPrimaries::Other(tag) = &result.color.primaries {
        assert!(["smpte428", "ebu3213"].contains(&tag.as_str()));
    }
    if let ColorTransfer::Other(tag) = &result.color.transfer {
        assert!(
            [
                "smpte240m",
                "log_sqrt",
                "iec61966_2_4",
                "bt1361_ecg",
                "bt2020_10",
                "bt2020_12",
                "smpte428",
            ]
            .contains(&tag.as_str())
        );
    }
    if let ColorMatrix::Other(tag) = &result.color.matrix {
        assert!(
            [
                "bt470bg",
                "fcc",
                "smpte2085",
                "ipt_c2",
                "ycgco_re",
                "ycgco_ro"
            ]
            .contains(&tag.as_str())
        );
    }
    assert!(!matches!(result.color.range, ColorRange::Other(_)));
    assert!(!matches!(
        result.color.white_point,
        ColorWhitePoint::Other(_)
    ));
    assert!(!matches!(
        result.color.provenance,
        ColorProvenance::Other(_)
    ));
    assert!(!matches!(result.color.bit_depth, ColorBitDepth::Other(_)));
}

proptest! {
    #[test]
    fn accepted_probe_metadata_satisfies_the_boundary_table(
        signed in prop_oneof![any::<i64>(), -4..4_i64, Just(604_800_000_000)],
        unsigned in prop_oneof![any::<u32>(), 0..32_u32, 4094..4099_u32, 8190..8195_u32],
        numerator in prop_oneof![any::<i32>(), -4..4_i32, 4094..4099_i32],
        denominator in -2..5_i32,
        confidence in prop_oneof![any::<u16>(), 9998..10003_u16],
        count in 0..20_usize,
    ) {
        for kind in [Kind::Image, Kind::Video, Kind::Audio, Kind::AudioVideo] {
            let sample = probe_kind(kind);
            assert_table(ValidatedProbe::new(sample.clone()).unwrap().metadata());
            let mutations: &[fn(&mut ProbeResult)] = &[
                |probe| probe.color.primaries = ColorPrimaries::Other("unlisted".into()),
                |probe| probe.color.transfer = ColorTransfer::Other("unlisted".into()),
                |probe| probe.color.matrix = ColorMatrix::Other("unlisted".into()),
                |probe| probe.color.range = ColorRange::Other("unlisted".into()),
                |probe| probe.color.white_point = ColorWhitePoint::Other("unlisted".into()),
                |probe| probe.color.bit_depth = ColorBitDepth::Other("unlisted".into()),
                |probe| probe.color.provenance = ColorProvenance::Other("unlisted".into()),
            ];
            let check = |mutate: &dyn Fn(&mut ProbeResult)| {
                let mut candidate = sample.clone();
                mutate(&mut candidate);
                if let Ok(accepted) = ValidatedProbe::new(candidate) {
                    assert_table(accepted.metadata());
                }
            };
            for mutate in mutations {
                check(mutate);
            }
            check(&|p| {
                p.streams.resize_with(count, || sample.streams[0].clone());
                for (index, stream) in p.streams.iter_mut().enumerate() {
                    stream.index = u32::try_from(index).unwrap();
                }
            });
            check(&|p| p.streams[0].index = unsigned);
            check(&|p| p.audio_stream = Some(unsigned));
            check(&|p| p.video_stream = Some(unsigned));
            check(&|p| p.streams[0].timestamp_origin = signed);
            check(&|p| p.streams[0].duration = signed);
            check(&|p| p.streams[0].time_base = [numerator, denominator]);
            check(&|p| p.streams[0].average_frame_rate = [numerator, denominator]);
            check(&|p| p.resolution = Some([unsigned, 2048]));
            check(&|p| p.resolution = Some([2048, unsigned]));
            check(&|p| p.duration_microseconds = signed);
            check(&|p| p.duration_frames = signed);
            check(&|p| p.fps = [unsigned, denominator.unsigned_abs()]);
            check(&|p| p.color.confidence_basis_points = confidence);
            check(&|p| p.color.bit_depth = ColorBitDepth::Integer(confidence % 36));
        }
    }
}
