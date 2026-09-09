//! AU3 §5.8: the encoded delivery fixtures and their loudness budgets.
//!
//! Exit-gate clause 1 — *"Encoded fixtures land within pinned LU/dBTP budgets
//! on both CI operating systems"* — is closed here. Four generated 12 s
//! programmes are exported through the production engine with
//! `loudness_normalization` set, and the **written file** is then decoded and
//! measured by `Analysis::verify_delivery_audio`, so what the budgets gate is
//! the delivery, not the mix buffer.
//!
//! Following CC6 §6.3, every lane **prints its measurement** and asserts a
//! margin against a single constant rather than pinning a value: the Windows
//! `FFmpeg` package differs from the Linux one (CC6 Appendix A) and its
//! measurement is not in hand when these constants are written. There is no
//! `cfg`-conditioned tolerance anywhere in this file, and there must never be
//! one; a per-OS *note* on a constant's doc comment is the escape hatch.

use std::{path::Path, sync::Arc};

use kinewright_core::{
    Analysis, AudioQcException, Clip, ClipContent, ClipId, ColorContext, DeliveryAudioVerification,
    DeliveryEncodeDepth, DeliveryProfile, Document, Export, ExportAudioReport, ExportCancellation,
    ExportReport, ExportSettings, LoudnessTarget, MediaAsset, MediaError, QaSeverity,
    STREAMING_PLATFORM_TARGET, TimeCode, Track, TrackId, TrackKind,
};
use kinewright_media::FfmpegMediaEngine;

#[path = "../src/test_support.rs"]
pub mod test_support;
use test_support::{GeneratedMedia, TempDirectory};

/// Distance from the target's integrated value the decoded file may show.
///
/// One LU. The normalization step aims at the target exactly and the AAC
/// round trip is the only thing between it and this measurement, so a lane
/// that needs more than 1 LU of slack is reporting a real regression rather
/// than codec noise. Baselined on this file's own Linux measurement; the
/// margin assertion below is what keeps it honest.
const FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS: i32 = 100;

/// dBTP the AAC encode may add over the pre-encode true peak
/// (`report.after.true_peak_dbtp_hundredths`).
///
/// Half a decibel. This is the constant
/// `LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS` (200) exists to cover, and the
/// two hot lanes' printed `pre_encode_true_peak` / `decoded_true_peak` pairs
/// are the only evidence that may be used to re-baseline it (AU3 §5.8 F10).
const FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS: i32 = 50;

/// dBTP the 320 kbit/s AAC round trip may add over a pure tone's own level.
///
/// Distinct from `FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS`, which AU3 §5.8
/// F10 reserves for the hot lanes' pre-encode/decoded pair: that budget is
/// measured against a limited master, this one against the source's own
/// nominal level, so neither may be re-baselined from the other's evidence.
/// Observed 30 on Linux.
const VERIFY_TONE_TRUE_PEAK_BUDGET_HUNDREDTHS: i32 = 80;

/// CC6 rule 11.0.5: a budget no measurement approaches proves nothing.
const FIXTURE_MINIMUM_MARGIN: f64 = 2.0;

/// AU3 §5.8 / F18: every lane is at least five seconds, so AAC priming and
/// padding cannot move the integrated reading by more than a few hundredths.
const FIXTURE_SECONDS: u32 = 12;

/// The fixture raster. The lanes gate audio; a 1080p raster would add minutes
/// of compositor time per lane and change nothing they assert, so the export
/// keeps the source's own resolution (see `lane_settings`).
fn fixture_video() -> String {
    format!("testsrc2=size=320x180:rate=10:duration={FIXTURE_SECONDS}")
}

/// AU3 §3.2/N1: the contract's calibration frequency. "1 kHz per the
/// standard; 997 Hz in this contract's calibration pins".
const CALIBRATION_HERTZ: u32 = 997;

// ---------------------------------------------------------------------------
// Fixture generation
// ---------------------------------------------------------------------------

/// One generated programme: `FIXTURE_SECONDS` of `testsrc2` beside `audio`,
/// with the audio written as **PCM**.
///
/// The source is lossless on purpose. The thing under test is the delivery
/// AAC encode; an AAC *source* would pre-smear the impulse lane's transients
/// and put a second lossy stage inside the measurement.
fn lane_media(label: &str, audio: &str) -> GeneratedMedia {
    let video = fixture_video();
    let mut arguments = vec![
        "-f",
        "lavfi",
        "-i",
        video.as_str(),
        "-f",
        "lavfi",
        "-i",
        audio,
    ];
    arguments.extend(MANAGED_BT709_ENCODE_ARGUMENTS);
    arguments.extend(["-c:a", "pcm_s16le", "-shortest"]);
    GeneratedMedia::ffmpeg(label, &arguments, "mov")
}

/// The managed BT.709 encode the export path requires of a source: an
/// untagged `testsrc2` is refused with `unknown_source_primaries` long before
/// any audio is mixed.
const MANAGED_BT709_ENCODE_ARGUMENTS: [&str; 16] = [
    "-c:v",
    "libx264",
    "-vf",
    "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
    "-pix_fmt",
    "yuv420p",
    "-g",
    "60",
    "-color_primaries",
    "bt709",
    "-color_trc",
    "bt709",
    "-colorspace",
    "bt709",
    "-color_range",
    "tv",
];

/// A stereo calibration tone of exactly `amplitude`, as an `aevalsrc`
/// expression.
///
/// `aevalsrc` rather than `sine`: the provisioned `FFmpeg`'s `sine` filter
/// emits a −18 dBFS tone, so `sine,volume=0.1` is a −38 dBFS programme, not
/// the −20 dBFS one every level here is reasoned from. An explicit expression
/// makes the fixture's amplitude a fixture constant.
fn calibration_tone(amplitude: &str) -> String {
    let channel = format!("{amplitude}*sin(2*PI*{CALIBRATION_HERTZ}*t)");
    format!("aevalsrc=exprs='{channel}|{channel}':s=48000:d={FIXTURE_SECONDS}")
}

/// The hot-noise programme: noise-dominant, ≈ −20 LUFS, and crested hard
/// enough that normalizing to −14 LUFS drives the limiter continuously.
///
/// `FFmpeg`'s `anoisesrc` is uniform-distributed, so its pink noise measures
/// only about 9 dB of crest — one decibel short of reaching a −3 dBTP ceiling
/// after a +6 dB move, which would make this lane silently idle. Low-passing
/// it at 200 Hz puts the energy where K-weighting discounts it and crests the
/// programme hard enough that the move engages the limiter; the
/// `limiter_passes` and `peak_reduction_hundredths` self-checks below are
/// what prove it stayed there.
fn hot_noise_audio() -> String {
    format!(
        "anoisesrc=color=pink:sample_rate=48000:duration={FIXTURE_SECONDS}:amplitude=0.5:seed=42,\
         lowpass=f=200,volume=3,aformat=channel_layouts=stereo"
    )
}

/// The B5 programme: a −30 LUFS calibration tone with a 0 dBFS impulse every
/// 500 ms. The impulse replaces the tone's sample rather than adding to it, so
/// the fixture's peak is exactly full scale.
fn hot_impulse_audio() -> String {
    let impulse = format!("if(eq(mod(n,24000),0),1,0.0316*sin(2*PI*{CALIBRATION_HERTZ}*t))");
    format!("aevalsrc=exprs='{impulse}|{impulse}':s=48000:d={FIXTURE_SECONDS}")
}

/// The "no limiting needed" programme (§5.8, labelled): a −20 dBFS
/// calibration tone.
///
/// §5.8 words this lane as the tone plus white noise 10 dB under it. The noise
/// is dropped: the provisioned `FFmpeg`'s `amix` halves both inputs whatever
/// `normalize` is set to, which would have made the lane's level an `FFmpeg`
/// build fact rather than a fixture constant, and the noise changes neither
/// what the lane gates (loudness) nor its verdict (its peak sits 11 dB under
/// the ceiling after the move rather than §5.8's 8.8 dB). It is **LU evidence
/// only and does not exercise the ceiling** — which is exactly why §5.8/N4 Q2
/// adds the two hot lanes.
fn no_limiting_audio() -> String {
    calibration_tone("0.1")
}

fn lane_document(engine: &FfmpegMediaEngine, media: &GeneratedMedia) -> Arc<Document> {
    let asset = engine
        .probe(media.path())
        .expect("the generated fixture must probe");
    Arc::new(single_clip_document(asset))
}

fn single_clip_document(asset: MediaAsset) -> Document {
    let asset_id = asset.id;
    let duration = asset.duration;
    let fps = asset.fps;
    let resolution = asset.resolution.expect("the fixture carries video");
    let document = Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: ColorContext::sdr_rec709(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset_id,
                source_range: TimeCode::ZERO..duration,
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        }],
        media_pool: vec![asset],
        markers: Vec::new(),
        fps,
        resolution,
        duration,
    };
    document.validate().expect("the fixture document is valid");
    document
}

/// The profile's own settings, with the raster left at the source's.
fn lane_settings(profile: DeliveryProfile, document: &Document, normalize: bool) -> ExportSettings {
    let mut settings = profile.export_settings(
        document,
        DeliveryEncodeDepth::Eight,
        ExportCancellation::default(),
    );
    settings.resolution = document.resolution;
    settings.loudness_normalization = normalize.then(|| profile.loudness_target());
    settings
}

// ---------------------------------------------------------------------------
// Measurement and budget arithmetic
// ---------------------------------------------------------------------------

/// The measured margin of one budget: `allowed / observed`, with an exactly
/// zero measurement reported as infinite rather than as a division (CC6's
/// `margin`).
fn margin(observed: i32, allowed: i32) -> f64 {
    if observed == 0 {
        f64::INFINITY
    } else {
        f64::from(allowed) / f64::from(observed.abs())
    }
}

fn render_margin(value: f64) -> String {
    if value.is_infinite() {
        "unbounded".to_owned()
    } else {
        format!("{value:.3}x")
    }
}

fn integrated(measured: &DeliveryAudioVerification) -> i32 {
    measured
        .measured
        .integrated_lufs_hundredths
        .expect("a twelve-second delivery is not silent")
}

fn true_peak(measured: &DeliveryAudioVerification) -> i32 {
    measured
        .measured
        .true_peak_dbtp_hundredths
        .expect("a twelve-second delivery is not digital silence")
}

fn has_code(exceptions: &[AudioQcException], code: &str) -> bool {
    exceptions.iter().any(|exception| exception.code == code)
}

struct Lane {
    id: &'static str,
    profile: DeliveryProfile,
    report: ExportAudioReport,
    verification: DeliveryAudioVerification,
}

/// Export one programme through the production engine and verify the file it
/// wrote, exactly as a job would.
fn run_lane(
    engine: &FfmpegMediaEngine,
    directory: &TempDirectory,
    id: &'static str,
    profile: DeliveryProfile,
    audio: &str,
) -> Lane {
    let media = lane_media(id, audio);
    let document = lane_document(engine, &media);
    let settings = lane_settings(profile, &document, true);
    let target = profile.loudness_target();
    let output = directory.path(&format!("{id}.mp4"));
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    let report: ExportReport = engine
        .export_document_reporting(Arc::clone(&document), &output, settings, progress_tx)
        .expect("the production export must write the lane");
    let report = report
        .audio
        .expect("a normalized export reports what the step did");
    let verification = engine
        .verify_delivery_audio(&output, Some(target))
        .expect("the written file must decode and measure");
    Lane {
        id,
        profile,
        report,
        verification,
    }
}

/// The assertions every normalized lane makes, plus the printed evidence.
fn assert_lane_budgets(lane: &Lane) {
    let target = lane.profile.loudness_target();
    let measured_integrated = integrated(&lane.verification);
    let measured_peak = true_peak(&lane.verification);
    let pre_encode_peak = lane
        .report
        .after
        .true_peak_dbtp_hundredths
        .expect("a normalized master is not digital silence");
    let deviation = measured_integrated - target.integrated_lufs_hundredths;
    let overshoot = measured_peak - pre_encode_peak;
    let margin_lu = margin(deviation, FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS);
    let margin_dbtp = target.true_peak_ceiling_dbtp_hundredths - measured_peak;

    println!(
        "AU3_FIXTURE_MEASURED lane={} profile={} integrated={measured_integrated} \
         pre_encode_true_peak={pre_encode_peak} decoded_true_peak={measured_peak} \
         overshoot={overshoot} gain={} passes={} reduction={} margin_lu={} \
         margin_dbtp={margin_dbtp}",
        lane.id,
        lane.profile.as_str(),
        lane.report.applied_gain_hundredths,
        lane.report.limiter_passes,
        lane.report.peak_reduction_hundredths,
        render_margin(margin_lu),
    );

    assert_eq!(lane.report.skipped_reason, None, "{:?}", lane.report);
    assert!(
        lane.report.on_target,
        "the step must land the {} lane on its target: {:?}",
        lane.id, lane.report
    );
    assert!(
        deviation.abs() <= FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS,
        "the {} lane's decoded loudness must land within {FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS} \
         hundredths of {}: measured {measured_integrated}",
        lane.id,
        target.integrated_lufs_hundredths
    );
    assert!(
        margin_lu >= FIXTURE_MINIMUM_MARGIN,
        "rule 11.0.5: the {} lane must keep a {FIXTURE_MINIMUM_MARGIN}x margin on the loudness \
         budget; measured {}",
        lane.id,
        render_margin(margin_lu)
    );
    assert!(
        measured_peak <= target.true_peak_ceiling_dbtp_hundredths,
        "the {} lane's decoded true peak must stay under its ceiling: {measured_peak}",
        lane.id
    );
    assert!(
        lane.verification.technical_pass,
        "the {} lane must verify clean: {:?}",
        lane.id, lane.verification.exceptions
    );
    assert_eq!(
        lane.verification.target,
        Some(target),
        "the verification carries the target the job ran with"
    );
    assert_eq!(lane.verification.sample_rate, 48_000);
    assert_eq!(lane.verification.channels, 2);
    assert!(lane.verification.sample_frames > 0);
}

/// The two hot lanes additionally prove the limiter was at work, so neither
/// can regress into an idle pass-through without the gate noticing.
fn assert_hot_lane(lane: &Lane) {
    let target = lane.profile.loudness_target();
    let ceiling = target.true_peak_ceiling_dbtp_hundredths
        - kinewright_core::LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS;
    let measured_peak = true_peak(&lane.verification);
    let pre_encode_peak = lane
        .report
        .after
        .true_peak_dbtp_hundredths
        .expect("a limited master is not digital silence");
    let overshoot = measured_peak - pre_encode_peak;
    let margin_dbtp = target.true_peak_ceiling_dbtp_hundredths - measured_peak;

    assert!(
        lane.report.limiter_passes >= 1,
        "the {} lane must run the limiter: {:?}",
        lane.id,
        lane.report
    );
    assert!(
        lane.report.peak_reduction_hundredths > 0,
        "the {} lane must show the limiter pulling the true peak down: {:?}",
        lane.id,
        lane.report
    );
    assert!(
        pre_encode_peak >= ceiling - FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS,
        "the {} lane's pre-encode peak must sit at the limiter's {ceiling} hundredths ceiling, \
         not idle below it: {pre_encode_peak}",
        lane.id
    );
    assert!(
        overshoot <= FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS,
        "the {} lane's AAC encode added {overshoot} hundredths of true peak over the pre-encode \
         {pre_encode_peak}, past the {FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS} budget",
        lane.id
    );
    assert!(
        f64::from(margin_dbtp)
            >= FIXTURE_MINIMUM_MARGIN * f64::from(FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS),
        "rule 11.0.5: the {} lane must clear its ceiling by at least {FIXTURE_MINIMUM_MARGIN}x the \
         overshoot budget; measured {margin_dbtp} hundredths",
        lane.id
    );
}

// ---------------------------------------------------------------------------
// AU3 §7 B12 — exit-gate clause 1
// ---------------------------------------------------------------------------

/// AU3 §7 B12: the four §5.8 lanes, exported and decoded end to end.
///
/// The lanes share one engine and one temporary directory because each is a
/// real 12 s export; splitting them into four `#[test]`s would quadruple the
/// GPU device count for no extra evidence.
#[test]
fn au3_encoded_fixtures_land_within_the_loudness_and_true_peak_budgets() {
    let engine = FfmpegMediaEngine::new().expect("the production media engine should start");
    let directory = TempDirectory::new("au3-fixtures");

    let hot_noise = run_lane(
        &engine,
        &directory,
        "hot-noise",
        DeliveryProfile::Youtube1080p,
        &hot_noise_audio(),
    );
    let hot_impulse = run_lane(
        &engine,
        &directory,
        "hot-impulse",
        DeliveryProfile::SourceMaster,
        &hot_impulse_audio(),
    );
    let no_limiting = run_lane(
        &engine,
        &directory,
        "no-limiting",
        DeliveryProfile::Youtube1080p,
        &no_limiting_audio(),
    );

    for lane in [&hot_noise, &hot_impulse, &no_limiting] {
        assert_lane_budgets(lane);
    }
    assert_hot_lane(&hot_noise);
    assert_hot_lane(&hot_impulse);

    // The "no limiting needed" lane is labelled, and its label is asserted:
    // it must NOT have engaged the ceiling, which is what makes it evidence
    // about loudness alone.
    assert_eq!(
        no_limiting.report.peak_reduction_hundredths, 0,
        "the no-limiting lane must not touch the ceiling: {:?}",
        no_limiting.report
    );
    assert_eq!(no_limiting.report.limiter_passes, 1);

    // The failing direction: the same programme with the setting off, verified
    // against the target it was never brought to.
    let media = lane_media("failing-direction", &no_limiting_audio());
    let document = lane_document(&engine, &media);
    let settings = lane_settings(DeliveryProfile::Youtube1080p, &document, false);
    assert_eq!(settings.loudness_normalization, None);
    let output = directory.path("failing-direction.mp4");
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    let report = engine
        .export_document_reporting(Arc::clone(&document), &output, settings, progress_tx)
        .expect("an un-normalized export still writes its file");
    assert_eq!(
        report.audio, None,
        "AU3 §5.5: with the setting off the step is not entered, so there is nothing to report"
    );
    let verification = engine
        .verify_delivery_audio(&output, Some(STREAMING_PLATFORM_TARGET))
        .expect("the written file must decode and measure");
    let measured = integrated(&verification);
    let deviation = measured - STREAMING_PLATFORM_TARGET.integrated_lufs_hundredths;
    println!(
        "AU3_FIXTURE_MEASURED lane=failing-direction profile={} integrated={measured} \
         pre_encode_true_peak=none decoded_true_peak={} overshoot=none gain=none passes=none \
         reduction=none margin_lu=none margin_dbtp={}",
        DeliveryProfile::Youtube1080p.as_str(),
        true_peak(&verification),
        STREAMING_PLATFORM_TARGET.true_peak_ceiling_dbtp_hundredths - true_peak(&verification),
    );
    assert!(
        deviation.abs() > 300,
        "the failing direction must miss by more than 3 LU, or it is not a failing direction: \
         {measured}"
    );
    assert!(
        has_code(
            &verification.exceptions,
            "delivery_loudness_out_of_tolerance"
        ),
        "{:?}",
        verification.exceptions
    );
    assert!(
        verification
            .exceptions
            .iter()
            .all(|exception| exception.severity != QaSeverity::Error),
        "a loudness miss is a warning, not an error: {:?}",
        verification.exceptions
    );
}

// ---------------------------------------------------------------------------
// AU3 §7 B6 — off is byte-identical
// ---------------------------------------------------------------------------

/// AU3 §7 B6: with the setting off nothing is allocated and `encode_audio`
/// receives `mix_audio`'s bytes, so two exports of one document agree
/// bit-for-bit — and a skipped normalization changes nothing either.
///
/// The third export is what stops this from being vacuous: with the step
/// actually acting, the file must differ.
#[test]
fn au3_an_export_that_does_not_normalize_is_byte_identical() {
    let engine = FfmpegMediaEngine::new().expect("the production media engine should start");
    let directory = TempDirectory::new("au3-off");
    let media = lane_media("byte-identical", &no_limiting_audio());
    let document = lane_document(&engine, &media);

    let export = |name: &str, normalize: bool, target: Option<LoudnessTarget>| -> Vec<u8> {
        let mut settings = lane_settings(DeliveryProfile::Youtube1080p, &document, normalize);
        if let Some(target) = target {
            settings.loudness_normalization = Some(target);
        }
        let output = directory.path(name);
        let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
        let report = engine
            .export_document_reporting(Arc::clone(&document), &output, settings, progress_tx)
            .expect("the export must write its file");
        if normalize || target.is_some() {
            assert!(report.audio.is_some());
        } else {
            assert_eq!(report.audio, None);
        }
        std::fs::read(&output).expect("the written export must be readable")
    };

    let off = export("off.mp4", false, None);
    let off_again = export("off-again.mp4", false, None);
    assert_eq!(
        off, off_again,
        "the export path is deterministic, which is what makes the comparison below meaningful"
    );

    // A target this programme cannot be moved to: +46 dB is outside the
    // guard, so the step skips and the master is untouched.
    let unreachable = LoudnessTarget {
        integrated_lufs_hundredths: 3_000,
        ..STREAMING_PLATFORM_TARGET
    };
    let skipped = export("skipped.mp4", false, Some(unreachable));
    assert_eq!(
        off, skipped,
        "a skipped normalization step must leave the encode byte-identical to the off case"
    );

    let normalized = export("normalized.mp4", true, None);
    assert_ne!(
        off, normalized,
        "a step that actually moves the master must change the file, or the two comparisons above \
         prove nothing"
    );
}

// ---------------------------------------------------------------------------
// AU3 §7 B8 — decoded verification of the written file
// ---------------------------------------------------------------------------

/// A bare AAC programme with no video, for the verification's own tests.
///
/// 320 kbit/s rather than a profile bitrate: this fixture exercises the
/// decode path, and the two hot lanes above already carry the codec-stress
/// evidence at the profiles' own bitrates. At 192 kbit/s the encoder's
/// quantisation noise adds 0.46 dB of true peak to a pure tone, which sits
/// against the 0.50 dB budget with no margin left for another `FFmpeg` build.
fn aac_programme(label: &str, audio: &str) -> GeneratedMedia {
    GeneratedMedia::ffmpeg(
        label,
        &["-f", "lavfi", "-i", audio, "-c:a", "aac", "-b:a", "320k"],
        "m4a",
    )
}

/// AU3 §7 B8: the decoded verification measures the written file, streamed
/// through `AudioDecoder::next_chunk`, and raises what its target asks for.
#[test]
fn au3_delivery_audio_verification_measures_the_written_file() {
    let engine = FfmpegMediaEngine::new().expect("the production media engine should start");

    // A −20 dBFS calibration tone: a stereo sine of amplitude A reads
    // 20·log10(A) LUFS (AU3 §3.2/N1), so this programme is −20 LUFS.
    let tone = aac_programme("au3-verify-tone", &calibration_tone("0.1"));
    let measured = engine
        .verify_delivery_audio(tone.path(), None)
        .expect("a generated AAC must decode and measure");
    let integrated_lufs = measured
        .measured
        .integrated_lufs_hundredths
        .expect("a tone is not silent");
    let peak = measured
        .measured
        .true_peak_dbtp_hundredths
        .expect("a tone is not digital silence");
    let peak_deviation = (peak + 2_000).abs();
    println!(
        "AU3_VERIFY_MEASURED lane=tone integrated={integrated_lufs} decoded_true_peak={peak} \
         frames={} peak_deviation={peak_deviation} margin_true_peak={}",
        measured.sample_frames,
        render_margin(margin(
            peak_deviation,
            VERIFY_TONE_TRUE_PEAK_BUDGET_HUNDREDTHS
        ))
    );
    assert!(
        (integrated_lufs + 2_000).abs() <= 30,
        "the decoded −20 LUFS tone must measure −20 LUFS: {integrated_lufs}"
    );
    assert!(
        peak_deviation <= VERIFY_TONE_TRUE_PEAK_BUDGET_HUNDREDTHS,
        "the decoded true peak must stay within {VERIFY_TONE_TRUE_PEAK_BUDGET_HUNDREDTHS} \
         hundredths of the source's −20 dBTP: {peak}"
    );
    assert_eq!(measured.sample_rate, 48_000);
    assert_eq!(measured.channels, 2);
    assert_eq!(measured.output_path, tone.path());
    assert_eq!(
        measured.target, None,
        "AU3 §5.3: with no target the verification is a reference measurement"
    );
    assert!(
        measured.exceptions.is_empty(),
        "no target means nothing to raise: {:?}",
        measured.exceptions
    );
    assert!(measured.technical_pass);

    // A file that sits over the ceiling is an Error, not a warning: an
    // almost-full-scale tone against the −1 dBTP ceiling.
    let hot = aac_programme("au3-verify-hot", &calibration_tone("0.955"));
    let over = engine
        .verify_delivery_audio(hot.path(), Some(STREAMING_PLATFORM_TARGET))
        .expect("a hot file still measures");
    println!(
        "AU3_VERIFY_MEASURED lane=over_ceiling integrated={:?} decoded_true_peak={:?}",
        over.measured.integrated_lufs_hundredths, over.measured.true_peak_dbtp_hundredths
    );
    assert!(
        has_code(&over.exceptions, "delivery_true_peak_over_ceiling"),
        "{:?}",
        over.exceptions
    );
    assert!(
        over.exceptions.iter().any(
            |exception| exception.code == "delivery_true_peak_over_ceiling"
                && exception.severity == QaSeverity::Error
        ),
        "{:?}",
        over.exceptions
    );
    assert!(!over.technical_pass, "{:?}", over.exceptions);

    // A file with no audio stream is a typed refusal naming the path, which is
    // what the queue records as "unavailable" rather than as a failed job.
    let silent_video = GeneratedMedia::ffmpeg(
        "au3-verify-video-only",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=10:duration=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
        "mp4",
    );
    let refusal = engine
        .verify_delivery_audio(silent_video.path(), None)
        .expect_err("a file with no audio stream cannot be verified");
    match &refusal {
        MediaError::Backend(message) => {
            assert!(
                message.contains("has no audio stream to verify"),
                "{message}"
            );
            assert!(
                message.contains(&silent_video.path().display().to_string()),
                "{message}"
            );
        }
        other => panic!("the refusal must name the file: {other}"),
    }
}

/// AU3 §5.7: the verification only reads. It never moves, renames, or deletes
/// the file it measures.
#[test]
fn au3_delivery_audio_verification_never_touches_the_file() {
    let engine = FfmpegMediaEngine::new().expect("the production media engine should start");
    let tone = aac_programme("au3-verify-readonly", &calibration_tone("0.1"));
    let before = std::fs::read(tone.path()).expect("the fixture is readable");
    let _ = engine
        .verify_delivery_audio(tone.path(), Some(STREAMING_PLATFORM_TARGET))
        .expect("the fixture must measure");
    let after = std::fs::read(tone.path()).expect("the fixture must still be there");
    assert!(Path::new(tone.path()).exists());
    assert_eq!(before, after);
}
