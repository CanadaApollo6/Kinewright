//! AU4 clip envelopes and automation — core contract tests (AU4 §7 items A1 to
//! A8 plus the core halves of A13, A16 and A20).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crossbeam_channel::Receiver;
use kinewright_core::{
    AssetId, AudioBus, AudioBusId, AudioMaster, AudioMix, AutomationCurve, Clip, ClipId,
    ColorContext, Document, ENVELOPE_DISPLAY_MIN_TENTH_DB, Effect, EffectId, FrameTexture,
    Keyframe, KeyframeInterpolation, MediaAsset, MediaCatalog, MediaEvent, MediaKind,
    MediaSourceFingerprint, OpError, Operation, ParamValue, Playback, Rational, RelinkCandidate,
    TRACK_AUTOMATION_PARAMETERS, TimeCode, Track, TrackId, TrackKind, TrackMix,
    clamp_project_curve, envelope_coalesce_key, is_hold_only_parameter, qa_document,
    rebase_clip_curve, track_automation_coalesce_key,
};

// ---------------------------------------------------------------------------
// builders
// ---------------------------------------------------------------------------

fn fps() -> Rational {
    Rational::new(30, 1).unwrap()
}

fn asset(id: u64, name: &str, duration: i64, rate: Rational) -> MediaAsset {
    MediaAsset {
        id: AssetId(id),
        path: std::path::PathBuf::from(format!("{name}.mp4")),
        name: name.to_owned(),
        duration: TimeCode(duration),
        fps: rate,
        kind: MediaKind::Video,
        resolution: Some((1_920, 1_080)),
        source_fingerprint: MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    }
}

/// An empty 30 fps project with one video track and one 300-frame asset.
fn empty_document() -> Document {
    let mut doc = Document {
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        }],
        media_pool: Vec::new(),
        markers: Vec::new(),
        fps: fps(),
        resolution: (1_920, 1_080),
        duration: TimeCode::ZERO,
    };
    Operation::AddAsset {
        asset: asset(1, "asset-1", 300, fps()),
    }
    .apply(&mut doc)
    .unwrap();
    doc
}

/// One 60-frame clip at the head of track 1.
fn document_with_one_clip() -> Document {
    let mut doc = empty_document();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(60),
    }
    .apply(&mut doc)
    .unwrap();
    doc
}

/// Three abutting 60-frame clips, so slide and roll have their neighbours.
fn document_with_three_clips() -> Document {
    let mut doc = empty_document();
    for index in 0..3 {
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at: TimeCode(index * 60),
            source: TimeCode(index * 60)..TimeCode(index * 60 + 60),
        }
        .apply(&mut doc)
        .unwrap();
    }
    doc
}

fn linear(points: &[(i64, i64)]) -> AutomationCurve {
    AutomationCurve {
        keyframes: points
            .iter()
            .map(|(at, value)| Keyframe {
                at: TimeCode(*at),
                value: *value,
                interpolation: KeyframeInterpolation::Linear,
            })
            .collect(),
    }
}

fn shaped(points: &[(i64, i64, KeyframeInterpolation)]) -> AutomationCurve {
    AutomationCurve {
        keyframes: points
            .iter()
            .map(|(at, value, interpolation)| Keyframe {
                at: TimeCode(*at),
                value: *value,
                interpolation: *interpolation,
            })
            .collect(),
    }
}

fn clip(doc: &Document, id: ClipId) -> &Clip {
    doc.tracks
        .iter()
        .flat_map(|track| &track.clips)
        .find(|clip| clip.id == id)
        .expect("clip")
}

fn envelope(doc: &Document, id: ClipId) -> AutomationCurve {
    clip(doc, id).audio_gain_curve.clone().expect("envelope")
}

/// The clip envelope read at an absolute **project** frame, which is what the
/// mix path hears.
fn envelope_at(doc: &Document, id: ClipId, project_frame: i64) -> i64 {
    let clip = clip(doc, id);
    let local = TimeCode(project_frame - clip.timeline_start.0);
    clip.audio_gain_curve
        .as_ref()
        .expect("envelope")
        .value_at(local)
        .expect("value")
}

fn set_envelope(doc: &mut Document, id: ClipId, curve: &AutomationCurve) {
    Operation::SetClipGainEnvelope {
        clip: id,
        curve: Some(curve.clone()),
    }
    .apply(doc)
    .unwrap();
}

fn set_track_curve(doc: &mut Document, track: TrackId, parameter: &str, curve: &AutomationCurve) {
    Operation::SetTrackAutomation {
        track,
        parameter: parameter.to_owned(),
        curve: Some(curve.clone()),
    }
    .apply(doc)
    .unwrap();
}

fn track_entry(doc: &Document, track: TrackId) -> Option<&TrackMix> {
    doc.audio_mix
        .tracks
        .iter()
        .find(|entry| entry.track == track)
}

fn keyed_effect(id: u64, name: &str, curve: &AutomationCurve) -> Effect {
    Effect {
        id: EffectId(id),
        name: "audio_eq".to_owned(),
        parameters: BTreeMap::new(),
        keyframes: BTreeMap::from([(name.to_owned(), curve.clone())]),
    }
}

// ---------------------------------------------------------------------------
// A1 — serde shape
// ---------------------------------------------------------------------------

/// AU4 §7 item A1.
#[test]
fn the_five_curve_fields_are_absent_by_default_and_round_trip_when_present() {
    let mut doc = document_with_one_clip();
    doc.audio_mix.tracks.push(TrackMix {
        track: TrackId(1),
        gain_tenth_db: -30,
        pan_percent: 0,
        mute: false,
        solo: false,
        gain_curve: None,
        pan_curve: None,
    });
    doc.audio_mix.buses.push(AudioBus {
        id: AudioBusId(1),
        name: "Dialogue".to_owned(),
        tracks: vec![TrackId(1)],
        gain_tenth_db: -20,
        effects: Vec::new(),
        ducking_sidechain_tracks: Vec::new(),
        gain_curve: None,
    });
    doc.audio_mix.master.gain_tenth_db = -10;
    doc.validate().unwrap();

    // Every new field is `Option::is_none`-skipped, so a curve-free document
    // carries none of the five keys and every pre-AU4 golden is byte-unchanged.
    let json = serde_json::to_string(&doc).unwrap();
    for key in ["audio_gain_curve", "gain_curve", "pan_curve"] {
        assert!(
            !json.contains(key),
            "a curve-free document must not carry {key} on the wire"
        );
    }

    // A pre-AU4 JSON deserialises with all five `None`.
    let round_tripped: Document = serde_json::from_str(&json).unwrap();
    assert!(clip(&round_tripped, ClipId(1)).audio_gain_curve.is_none());
    let entry = track_entry(&round_tripped, TrackId(1)).unwrap();
    assert!(entry.gain_curve.is_none() && entry.pan_curve.is_none());
    assert!(round_tripped.audio_mix.buses[0].gain_curve.is_none());
    assert!(round_tripped.audio_mix.master.gain_curve.is_none());
    assert_eq!(round_tripped, doc);

    // A curve-bearing document round-trips exactly.
    let curve = linear(&[(0, 0), (30, -120)]);
    let mut curved = doc.clone();
    set_envelope(&mut curved, ClipId(1), &curve);
    set_track_curve(&mut curved, TrackId(1), "gain_tenth_db", &curve);
    set_track_curve(&mut curved, TrackId(1), "pan_percent", &linear(&[(0, -50)]));
    curved.audio_mix.buses[0].gain_curve = Some(curve.clone());
    curved.audio_mix.master.gain_curve = Some(curve.clone());
    curved.validate().unwrap();
    let json = serde_json::to_string(&curved).unwrap();
    assert_eq!(serde_json::from_str::<Document>(&json).unwrap(), curved);
}

// ---------------------------------------------------------------------------
// A2 — the `Copy` loss and the widened neutrality
// ---------------------------------------------------------------------------

struct CopyProbe<T>(std::marker::PhantomData<T>);

trait ProbeByClone {
    fn is_copy(&self) -> bool {
        false
    }
}

impl<T: Clone> ProbeByClone for CopyProbe<T> {}

impl<T: Copy> CopyProbe<T> {
    #[allow(clippy::unused_self)]
    fn is_copy(&self) -> bool {
        true
    }
}

/// Rule 5's four `const` claims, proven by calling each from a `const fn`.
/// A `const` *item* of a type with a destructor cannot be evaluated at all
/// once `TrackMix` owns a `Vec`, so this is the proof that survives the
/// `Copy` loss.
const fn probe_neutral(track: TrackId) -> TrackMix {
    TrackMix::neutral(track)
}
const fn probe_track_is_neutral(mix: &TrackMix) -> bool {
    mix.is_neutral()
}
const fn probe_master_is_neutral(master: &AudioMaster) -> bool {
    master.is_neutral()
}
const fn probe_mix_is_empty(mix: &AudioMix) -> bool {
    mix.is_empty()
}

/// AU4 §7 item A2.
#[test]
fn track_mix_is_clone_not_copy_and_a_curve_defeats_neutrality() {
    // Rule 5: `Clone + PartialEq + Eq`, and **not** `Copy`. The inherent
    // method wins whenever the `Copy` bound holds, so the probe reports the
    // real answer rather than a compile error either way.
    fn assert_clone_eq<T: Clone + PartialEq + Eq>() {}
    assert_clone_eq::<TrackMix>();
    assert!(
        !CopyProbe::<TrackMix>(std::marker::PhantomData).is_copy(),
        "TrackMix must lose Copy"
    );
    assert!(
        CopyProbe::<TimeCode>(std::marker::PhantomData).is_copy(),
        "the probe reports Copy when the bound holds"
    );

    // Rule 5: the four `const` claims survive; see the probes above.
    let neutral = probe_neutral(TrackId(7));
    assert!(probe_track_is_neutral(&neutral));
    assert!(neutral.gain_curve.is_none() && neutral.pan_curve.is_none());
    assert!(probe_master_is_neutral(&AudioMaster::default()));
    assert!(probe_mix_is_empty(&AudioMix::default()));

    // Rule 11: a neutral-scalar entry carrying a curve is *not* neutral, so it
    // is never elided off the wire and never removed by the `retain` arm.
    let mut doc = document_with_one_clip();
    set_track_curve(&mut doc, TrackId(1), "pan_percent", &linear(&[(0, 40)]));
    let entry = track_entry(&doc, TrackId(1)).expect("the entry survives");
    assert_eq!(entry.gain_tenth_db, 0);
    assert_eq!(entry.pan_percent, 0);
    assert!(!entry.is_neutral());
    assert!(!doc.audio_mix.is_empty());
    let json = serde_json::to_string(&doc).unwrap();
    assert!(json.contains("pan_curve"));
    assert_eq!(serde_json::from_str::<Document>(&json).unwrap(), doc);
}

// ---------------------------------------------------------------------------
// A3 — resolution, the two constants, the coalesce keys, the two predicates
// ---------------------------------------------------------------------------

/// AU4 §7 item A3.
#[test]
fn resolution_the_display_bound_and_the_two_coalesce_keys_are_exact() {
    // Rule 9: a curve replaces its scalar; the scalar is the parked value and
    // is left exactly where it was.
    let mut doc = document_with_one_clip();
    Operation::SetClipAudio {
        clip: ClipId(1),
        gain_tenth_db: -60,
        fade_in_frames: TimeCode::ZERO,
        fade_out_frames: TimeCode::ZERO,
    }
    .apply(&mut doc)
    .unwrap();
    let curve = linear(&[(0, 0), (30, -300)]);
    set_envelope(&mut doc, ClipId(1), &curve);
    assert_eq!(clip(&doc, ClipId(1)).audio_gain_tenth_db, -60);
    assert_eq!(envelope_at(&doc, ClipId(1), 15), -150);
    assert_eq!(envelope_at(&doc, ClipId(1), 45), -300);

    // Rule 12: `value_at` returning `None` falls back to the static scalar and
    // never panics.
    let empty = AutomationCurve {
        keyframes: Vec::new(),
    };
    assert_eq!(empty.value_at(TimeCode(5)), None);
    assert_eq!(empty.value_at(TimeCode(5)).unwrap_or(-60), -60);

    // Rule 30 and §5.1's display bound.
    assert_eq!(
        TRACK_AUTOMATION_PARAMETERS,
        ["gain_tenth_db", "pan_percent"]
    );
    assert_eq!(ENVELOPE_DISPLAY_MIN_TENTH_DB, -400);

    // Rule 35's two spellings.
    assert_eq!(envelope_coalesce_key(ClipId(4)), "envelope:4");
    assert_eq!(
        track_automation_coalesce_key(TrackId(2), "pan_percent"),
        "track_automation:2:pan_percent"
    );
    assert_eq!(
        track_automation_coalesce_key(TrackId(2), "gain_tenth_db"),
        "track_automation:2:gain_tenth_db"
    );

    // Rule 38: `is_hold_only_parameter` is `pub`, is re-exported, and AU4 adds
    // no entry to it — none of the five owners is a switch, so all five
    // interpolations stay legal on every one of them.
    for switch in ["bypass", "detector", "true_peak"] {
        assert!(is_hold_only_parameter("audio_compressor", switch));
    }
    for ride in TRACK_AUTOMATION_PARAMETERS {
        assert!(!is_hold_only_parameter("audio_compressor", ride));
        assert!(!is_hold_only_parameter("audio_eq", ride));
        // None of the five owners is latency-bearing either, so
        // `is_static_audio_parameter` gains no entry.
        assert!(!kinewright_core::is_static_audio_parameter(
            "audio_compressor",
            ride
        ));
        assert!(!kinewright_core::is_static_audio_parameter(
            "audio_gain",
            ride
        ));
    }
    assert!(kinewright_core::is_static_audio_parameter(
        "audio_compressor",
        "lookahead_milliseconds"
    ));
    let eased = shaped(&[
        (0, 0, KeyframeInterpolation::EaseInOut),
        (30, -200, KeyframeInterpolation::EaseIn),
        (45, -100, KeyframeInterpolation::Hold),
    ]);
    set_envelope(&mut doc, ClipId(1), &eased);
    set_track_curve(&mut doc, TrackId(1), "gain_tenth_db", &eased);
    doc.validate().unwrap();
}

/// AU4 §7 item A3: the display bound is a *display* bound.
#[test]
fn no_validation_path_reads_the_envelope_display_minimum() {
    // A key below `ENVELOPE_DISPLAY_MIN_TENTH_DB` but inside the validated
    // domain is legal everywhere: in the two operations, in `Document::validate`
    // and in the document invariant reached through a hand edit.
    let below = i64::from(ENVELOPE_DISPLAY_MIN_TENTH_DB) - 100;
    let curve = linear(&[(0, below), (30, -600)]);

    let mut doc = document_with_one_clip();
    set_envelope(&mut doc, ClipId(1), &curve);
    set_track_curve(&mut doc, TrackId(1), "gain_tenth_db", &curve);
    doc.audio_mix.master.gain_curve = Some(curve.clone());
    doc.audio_mix.buses.push(AudioBus {
        id: AudioBusId(1),
        name: "Music".to_owned(),
        tracks: vec![TrackId(1)],
        gain_tenth_db: 0,
        effects: Vec::new(),
        ducking_sidechain_tracks: Vec::new(),
        gain_curve: Some(curve.clone()),
    });
    doc.validate().unwrap();

    // And a value one tenth-dB past the *validated* floor is rejected, so the
    // test is not passing for want of any check at all.
    let outside = linear(&[(0, -601)]);
    assert!(matches!(
        Operation::SetClipGainEnvelope {
            clip: ClipId(1),
            curve: Some(outside),
        }
        .apply(&mut doc),
        Err(OpError::ClipGainEnvelopeOutOfRange { value: -601, .. })
    ));
}

// ---------------------------------------------------------------------------
// A5 — the merge and the entry lifecycle
// ---------------------------------------------------------------------------

/// AU4 §7 item A5.
#[test]
fn set_track_mix_merges_the_stored_curves_and_never_drops_a_ride() {
    let mut doc = document_with_one_clip();
    let gain = linear(&[(0, -100), (30, -300)]);
    let pan = linear(&[(0, -50), (30, 50)]);
    set_track_curve(&mut doc, TrackId(1), "gain_tenth_db", &gain);
    set_track_curve(&mut doc, TrackId(1), "pan_percent", &pan);

    // Rule 32: one nudge of a fader keeps both rides byte-identical.
    Operation::SetTrackMix {
        track: TrackId(1),
        gain_tenth_db: -40,
        pan_percent: 10,
        mute: false,
        solo: false,
    }
    .apply(&mut doc)
    .unwrap();
    let entry = track_entry(&doc, TrackId(1)).expect("the entry survives a fader nudge");
    assert_eq!(entry.gain_tenth_db, -40);
    assert_eq!(entry.gain_curve.as_ref(), Some(&gain));
    assert_eq!(entry.pan_curve.as_ref(), Some(&pan));

    // Rule 33: the strip's `Reset` is a neutral `SetTrackMix`, and it keeps the
    // rides rather than deleting them through the `retain` arm.
    Operation::SetTrackMix {
        track: TrackId(1),
        gain_tenth_db: 0,
        pan_percent: 0,
        mute: false,
        solo: false,
    }
    .apply(&mut doc)
    .unwrap();
    let entry = track_entry(&doc, TrackId(1)).expect("a neutral set keeps the automated entry");
    assert_eq!(entry.gain_tenth_db, 0);
    assert_eq!(entry.gain_curve.as_ref(), Some(&gain));
    assert_eq!(entry.pan_curve.as_ref(), Some(&pan));
}

/// AU4 §7 item A5.
#[test]
fn set_track_automation_shares_the_track_mix_entry_lifecycle() {
    let pristine = document_with_one_clip();
    let pristine_json = serde_json::to_string(&pristine).unwrap();
    let mut doc = pristine.clone();

    // Rule 32a: an un-mixed track gets a **pushed** entry, re-sorted by track.
    Operation::AddTrack {
        track: Track {
            id: TrackId(4),
            kind: TrackKind::Audio,
            sync_lock: false,
            clips: Vec::new(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    let curve = linear(&[(0, -100), (30, -300)]);
    set_track_curve(&mut doc, TrackId(4), "gain_tenth_db", &curve);
    set_track_curve(
        &mut doc,
        TrackId(1),
        "pan_percent",
        &linear(&[(0, -40), (30, 40)]),
    );
    assert_eq!(
        doc.audio_mix
            .tracks
            .iter()
            .map(|entry| entry.track)
            .collect::<Vec<_>>(),
        vec![TrackId(1), TrackId(4)],
        "the vector stays sorted by track"
    );

    // Rule 27: setting a curve equal to the stored one is accepted and
    // produces an identical document.
    let before = doc.clone();
    set_track_curve(&mut doc, TrackId(4), "gain_tenth_db", &curve);
    assert_eq!(doc, before);

    // Rule 32a: clearing the last curve on an otherwise neutral track removes
    // the entry, leaving the document byte-identical to one that never had it.
    for (track, parameter) in [(TrackId(4), "gain_tenth_db"), (TrackId(1), "pan_percent")] {
        Operation::SetTrackAutomation {
            track,
            parameter: parameter.to_owned(),
            curve: None,
        }
        .apply(&mut doc)
        .unwrap();
    }
    assert!(doc.audio_mix.tracks.is_empty());
    Operation::RemoveTrack { track: TrackId(4) }
        .apply(&mut doc)
        .unwrap();
    assert_eq!(serde_json::to_string(&doc).unwrap(), pristine_json);

    // A track carrying an off-neutral scalar keeps its entry when the last
    // curve goes: only a fully neutral entry is elided.
    Operation::SetTrackMix {
        track: TrackId(1),
        gain_tenth_db: -55,
        pan_percent: 0,
        mute: false,
        solo: false,
    }
    .apply(&mut doc)
    .unwrap();
    set_track_curve(&mut doc, TrackId(1), "gain_tenth_db", &curve);
    Operation::SetTrackAutomation {
        track: TrackId(1),
        parameter: "gain_tenth_db".to_owned(),
        curve: None,
    }
    .apply(&mut doc)
    .unwrap();
    let entry = track_entry(&doc, TrackId(1)).expect("an off-neutral scalar keeps the entry");
    assert_eq!(entry.gain_tenth_db, -55);
    assert!(entry.gain_curve.is_none());
}

/// AU4 §7 item A5.
#[test]
fn hand_edited_track_automation_is_rejected_during_document_validation() {
    let mut doc = document_with_one_clip();
    doc.audio_mix.tracks.push(TrackMix {
        track: TrackId(1),
        gain_tenth_db: 0,
        pan_percent: 0,
        mute: false,
        solo: false,
        gain_curve: None,
        // The project is 60 frames, so frame 60 is one past the last.
        pan_curve: Some(linear(&[(0, 0), (60, 50)])),
    });
    assert!(matches!(
        doc.validate(),
        Err(OpError::TrackAutomationKeyframeOutsideProject {
            at: TimeCode(60),
            duration: TimeCode(60),
            ..
        })
    ));

    // Rule 39: an unknown parameter is rejected before the curve is validated,
    // so a typo gets the vocabulary back rather than a structural complaint.
    let mut doc = document_with_one_clip();
    assert_eq!(
        Operation::SetTrackAutomation {
            track: TrackId(1),
            parameter: "gain".to_owned(),
            curve: Some(AutomationCurve {
                keyframes: Vec::new(),
            }),
        }
        .apply(&mut doc),
        Err(OpError::UnknownTrackAutomationParameter {
            parameter: "gain".to_owned(),
        })
    );
}

// ---------------------------------------------------------------------------
// A6 — `rebase_clip_curve`
// ---------------------------------------------------------------------------

/// AU4 §7 item A6.
#[test]
fn rebase_preserves_both_boundary_values_exactly_and_holds_exactly() {
    // Both formulas, and they are not the same expression: the left value is
    // `value_at(delta_local)` and the right `value_at(delta_local + n - 1)`.
    let curve = linear(&[(0, 0), (100, 1_000)]);
    let rebased = rebase_clip_curve(&curve, TimeCode(20), TimeCode(50));
    assert_eq!(rebased.value_at(TimeCode(0)), curve.value_at(TimeCode(20)));
    assert_eq!(rebased.value_at(TimeCode(49)), curve.value_at(TimeCode(69)));
    rebased.validate().unwrap();

    // A `Hold` segment is exact everywhere, not merely at the boundary.
    let held = shaped(&[
        (0, -200, KeyframeInterpolation::Hold),
        (40, -50, KeyframeInterpolation::Hold),
        (90, 0, KeyframeInterpolation::Linear),
    ]);
    let rebased = rebase_clip_curve(&held, TimeCode(10), TimeCode(60));
    for local in 0..60 {
        assert_eq!(
            rebased.value_at(TimeCode(local)),
            held.value_at(TimeCode(local + 10)),
            "a Hold segment is exact at local frame {local}"
        );
    }

    // A `Linear` segment is within one tenth-dB, which is `rounded_div`'s own
    // resolution.
    let rebased = rebase_clip_curve(&curve, TimeCode(0), TimeCode(50));
    for local in 0..50 {
        let before = curve.value_at(TimeCode(local)).unwrap();
        let after = rebased.value_at(TimeCode(local)).unwrap();
        assert!(
            (before - after).abs() <= 1,
            "linear frame {local}: {before} vs {after}"
        );
    }
}

/// AU4 §7 item A6: the printed counterexample.
#[test]
fn a_truncated_eased_segment_is_reshaped_and_the_move_is_printed() {
    let curve = shaped(&[
        (0, 0, KeyframeInterpolation::EaseInOut),
        (100, 1_000, KeyframeInterpolation::Linear),
    ]);
    let rebased = rebase_clip_curve(&curve, TimeCode::ZERO, TimeCode(50));

    // The boundary key is exact in both curves.
    assert_eq!(curve.value_at(TimeCode(49)), Some(485));
    assert_eq!(rebased.value_at(TimeCode(49)), Some(485));

    let before = curve.value_at(TimeCode(25)).unwrap();
    let after = rebased.value_at(TimeCode(25)).unwrap();
    println!(
        "AU4_EASED_RESHAPE frame=25 before={before} after={after} delta={}",
        after - before
    );
    assert_eq!(before, 156);
    assert_eq!(after, 250);
    assert_eq!(after - before, 94);
}

/// AU4 §7 item A6.
#[test]
fn rebase_inserts_nothing_when_lengthening_and_keeps_one_key_when_all_are_dropped() {
    let curve = linear(&[(10, -100), (20, -200)]);

    // Rule 15: a lengthening trim inserts nothing on the right, because
    // `value_at` already clamps past the last key.
    let longer = rebase_clip_curve(&curve, TimeCode::ZERO, TimeCode(400));
    assert_eq!(longer, curve);

    // Rule 13 step 5: an all-dropped curve keeps its single boundary key, so
    // the envelope becomes a constant equal to what was audible at that edge
    // and never falls back to the static scalar.
    let after = rebase_clip_curve(&curve, TimeCode(100), TimeCode(30));
    assert_eq!(after.keyframes.len(), 1);
    assert_eq!(after.keyframes[0].at, TimeCode::ZERO);
    assert_eq!(after.keyframes[0].value, -200);
    after.validate().unwrap();

    // Rule 13's negative branch: nothing is inserted on the left and the newly
    // exposed head is flat at the first key's value.
    let pulled_out = rebase_clip_curve(&curve, TimeCode(-15), TimeCode(60));
    assert_eq!(
        pulled_out
            .keyframes
            .iter()
            .map(|key| key.at.0)
            .collect::<Vec<_>>(),
        vec![25, 35]
    );
    for local in 0..25 {
        assert_eq!(pulled_out.value_at(TimeCode(local)), Some(-100));
    }

    // Rule 14: the dedupe never picks a wrong value. A survivor at local 0
    // means `delta_local` is exactly that key's frame, so the boundary key
    // carries the same value and the same forward interpolation.
    let colliding = shaped(&[
        (10, -100, KeyframeInterpolation::EaseIn),
        (20, -200, KeyframeInterpolation::Linear),
        (40, -300, KeyframeInterpolation::Linear),
    ]);
    let rebased = rebase_clip_curve(&colliding, TimeCode(10), TimeCode(11));
    assert_eq!(rebased.keyframes[0].value, -100);
    assert_eq!(
        rebased.keyframes[0].interpolation,
        KeyframeInterpolation::EaseIn
    );
    rebased.validate().unwrap();
}

// ---------------------------------------------------------------------------
// A7 — `clamp_project_curve` and `recompute_duration`
// ---------------------------------------------------------------------------

/// A project with two 60-frame clips, a track ride, a bus fader ride, a master
/// fader ride and a master `Effect.keyframes` curve, all keyed to frame 90.
fn automated_project() -> Document {
    let mut doc = empty_document();
    for index in 0..2 {
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at: TimeCode(index * 60),
            source: TimeCode(index * 60)..TimeCode(index * 60 + 60),
        }
        .apply(&mut doc)
        .unwrap();
    }
    // A clip envelope beside the four project-frame curves, so every
    // shortening operation below crosses both time bases in one apply.
    set_envelope(&mut doc, ClipId(2), &linear(&[(0, 0), (59, -590)]));
    let ride = linear(&[(0, 0), (90, -240)]);
    set_track_curve(&mut doc, TrackId(1), "gain_tenth_db", &ride);
    Operation::UpsertAudioBus {
        bus: AudioBus {
            id: AudioBusId(1),
            name: "Music".to_owned(),
            tracks: vec![TrackId(1)],
            gain_tenth_db: 0,
            effects: vec![keyed_effect(
                1,
                "low_gain_tenth_db",
                &linear(&[(0, 0), (90, 60)]),
            )],
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: Some(ride.clone()),
        },
    }
    .apply(&mut doc)
    .unwrap();
    Operation::SetAudioMaster {
        master: AudioMaster {
            gain_tenth_db: 0,
            gain_curve: Some(ride.clone()),
            effects: vec![keyed_effect(
                1,
                "low_gain_tenth_db",
                &linear(&[(0, 0), (90, 60)]),
            )],
        },
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.duration, TimeCode(120));
    doc
}

/// Every project-frame curve in the document, in a stable order.
fn project_curves(doc: &Document) -> Vec<AutomationCurve> {
    let mut curves = Vec::new();
    for entry in &doc.audio_mix.tracks {
        curves.extend(entry.gain_curve.clone());
        curves.extend(entry.pan_curve.clone());
    }
    for bus in &doc.audio_mix.buses {
        curves.extend(bus.gain_curve.clone());
        for effect in &bus.effects {
            curves.extend(effect.keyframes.values().cloned());
        }
    }
    curves.extend(doc.audio_mix.master.gain_curve.clone());
    for effect in &doc.audio_mix.master.effects {
        curves.extend(effect.keyframes.values().cloned());
    }
    curves
}

/// AU4 §7 item A7.
#[test]
fn shortening_the_project_clamps_every_project_frame_curve_instead_of_failing() {
    // Rule 19's four shortening operations, each on its own copy of the same
    // automated project. Every one of them keys past the new duration, and
    // every one of them previously failed.
    let shortenings: Vec<(&str, Operation, i64)> = vec![
        ("DeleteClip", Operation::DeleteClip { clip: ClipId(2) }, 60),
        (
            "TrimClip",
            Operation::TrimClip {
                clip: ClipId(2),
                new_source: TimeCode(60)..TimeCode(90),
            },
            90,
        ),
        (
            "RippleDeleteClip",
            Operation::RippleDeleteClip { clip: ClipId(2) },
            60,
        ),
        (
            "SetClipSpeed",
            Operation::SetClipSpeed {
                clip: ClipId(2),
                speed_percent: 200,
            },
            90,
        ),
    ];
    for (name, operation, expected) in shortenings {
        let before = automated_project();
        let audible: Vec<i64> = project_curves(&before)
            .iter()
            .map(|curve| curve.value_at(TimeCode(expected - 1)).unwrap())
            .collect();
        let mut doc = before.clone();
        operation
            .apply(&mut doc)
            .unwrap_or_else(|error| panic!("{name} must succeed, got {error}"));
        assert_eq!(doc.duration, TimeCode(expected), "{name}");
        let after = project_curves(&doc);
        assert_eq!(after.len(), audible.len(), "{name} keeps every curve");
        for (index, curve) in after.iter().enumerate() {
            curve.validate().unwrap();
            assert!(
                curve
                    .keyframes
                    .iter()
                    .all(|key| key.at < TimeCode(expected)),
                "{name} curve {index} still keys past the new duration"
            );
            assert_eq!(
                curve.value_at(TimeCode(expected - 1)).unwrap(),
                audible[index],
                "{name} changed the value audible at the new last frame of curve {index}"
            );
        }
    }
}

/// AU4 §7 item A7.
#[test]
fn deleting_the_only_clip_of_an_automated_project_succeeds_with_one_key_at_frame_zero() {
    let mut doc = empty_document();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(60),
    }
    .apply(&mut doc)
    .unwrap();
    let ride = linear(&[(0, -70), (30, -240)]);
    set_track_curve(&mut doc, TrackId(1), "gain_tenth_db", &ride);
    Operation::UpsertAudioBus {
        bus: AudioBus {
            id: AudioBusId(1),
            name: "Music".to_owned(),
            tracks: vec![TrackId(1)],
            gain_tenth_db: 0,
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: Some(ride.clone()),
        },
    }
    .apply(&mut doc)
    .unwrap();
    Operation::SetAudioMaster {
        master: AudioMaster {
            gain_tenth_db: 0,
            gain_curve: Some(ride.clone()),
            effects: vec![keyed_effect(
                1,
                "low_gain_tenth_db",
                &linear(&[(0, 20), (30, 60)]),
            )],
        },
    }
    .apply(&mut doc)
    .unwrap();

    Operation::DeleteClip { clip: ClipId(1) }
        .apply(&mut doc)
        .unwrap();
    assert_eq!(doc.duration, TimeCode::ZERO);
    let curves = project_curves(&doc);
    assert_eq!(curves.len(), 4);
    for curve in &curves {
        curve.validate().unwrap();
        assert_eq!(curve.keyframes.len(), 1, "reduced to one key");
        assert_eq!(curve.keyframes[0].at, TimeCode::ZERO);
    }
    // Each carries the value that was audible at frame 0.
    assert_eq!(curves[0].keyframes[0].value, -70);
    assert_eq!(curves[3].keyframes[0].value, 20);
    doc.validate().unwrap();
}

/// AU4 §7 item A7: rule 18's `duration < previous` guard is load-bearing.
#[test]
fn a_write_that_over_runs_an_unchanged_duration_is_still_rejected() {
    let base = automated_project();

    // An operation that does not shorten the project leaves every curve
    // byte-identical.
    let mut doc = base.clone();
    Operation::MoveClip {
        clip: ClipId(1),
        to_track: TrackId(1),
        to: TimeCode(0),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc, base);

    // Three agent-authored writes past the end, on an unchanged duration.
    let outside = linear(&[(0, 0), (120, -100)]);
    let mut doc = automated_project();
    assert!(matches!(
        Operation::UpsertAudioBus {
            bus: AudioBus {
                id: AudioBusId(1),
                name: "Music".to_owned(),
                tracks: vec![TrackId(1)],
                gain_tenth_db: 0,
                effects: vec![keyed_effect(
                    1,
                    "low_gain_tenth_db",
                    &linear(&[(0, 0), (120, 60)])
                )],
                ducking_sidechain_tracks: Vec::new(),
                gain_curve: None,
            },
        }
        .apply(&mut doc),
        Err(OpError::AudioBusKeyframeOutsideProject { .. })
    ));
    assert!(matches!(
        Operation::SetAudioMaster {
            master: AudioMaster {
                gain_tenth_db: 0,
                gain_curve: None,
                effects: vec![keyed_effect(
                    1,
                    "low_gain_tenth_db",
                    &linear(&[(0, 0), (120, 60)])
                )],
            },
        }
        .apply(&mut doc),
        Err(OpError::AudioMasterKeyframeOutsideProject { .. })
    ));
    assert!(matches!(
        Operation::SetTrackAutomation {
            track: TrackId(1),
            parameter: "gain_tenth_db".to_owned(),
            curve: Some(outside.clone()),
        }
        .apply(&mut doc),
        Err(OpError::TrackAutomationKeyframeOutsideProject {
            at: TimeCode(120),
            duration: TimeCode(120),
            ..
        })
    ));
    // The fader curves take the same relaxed bound and the same rejection.
    let mut hand_edited = automated_project();
    hand_edited.audio_mix.master.gain_curve = Some(outside.clone());
    assert!(matches!(
        hand_edited.validate(),
        Err(OpError::AudioMasterGainKeyframeOutsideProject {
            at: TimeCode(120),
            duration: TimeCode(120),
        })
    ));
    let mut hand_edited = automated_project();
    hand_edited.audio_mix.buses[0].gain_curve = Some(outside);
    assert!(matches!(
        hand_edited.validate(),
        Err(OpError::AudioBusGainKeyframeOutsideProject {
            at: TimeCode(120),
            duration: TimeCode(120),
            ..
        })
    ));
}

/// AU4 §7 item A7: `clamp_project_curve` on its own.
#[test]
fn clamp_project_curve_is_identity_inside_and_one_key_at_zero_on_an_empty_project() {
    let curve = linear(&[(0, 0), (30, -100), (59, -200)]);
    assert_eq!(clamp_project_curve(&curve, TimeCode(60)), curve);
    assert_eq!(clamp_project_curve(&curve, TimeCode(600)), curve);

    let clamped = clamp_project_curve(&curve, TimeCode(20));
    assert_eq!(
        clamped
            .keyframes
            .iter()
            .map(|key| key.at.0)
            .collect::<Vec<_>>(),
        vec![0, 19]
    );
    assert_eq!(clamped.value_at(TimeCode(19)), curve.value_at(TimeCode(19)));

    let empty_project = clamp_project_curve(&curve, TimeCode::ZERO);
    assert_eq!(empty_project.keyframes.len(), 1);
    assert_eq!(empty_project.keyframes[0].at, TimeCode::ZERO);
    assert_eq!(empty_project.keyframes[0].value, 0);
    assert_eq!(
        clamp_project_curve(&curve, TimeCode(-5)),
        empty_project,
        "a negative duration takes the same arm"
    );
}

// ---------------------------------------------------------------------------
// A8 — one test per §2.4 row
// ---------------------------------------------------------------------------

/// A ramp keyed across a whole 60-frame clip, plus the same ramp on a colour
/// node so the two clip-local time bases are proven to move together.
fn clip_with_envelope_and_colour_curve() -> Document {
    let mut doc = document_with_one_clip();
    let ramp = linear(&[(0, 0), (59, -590)]);
    set_envelope(&mut doc, ClipId(1), &ramp);
    Operation::AddEffect {
        clip: ClipId(1),
        effect: Effect {
            id: EffectId(1),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                ParamValue::Integer(0),
            )]),
            keyframes: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                linear(&[(0, 0), (59, 4_720)]),
            )]),
        },
    }
    .apply(&mut doc)
    .unwrap();
    doc
}

fn colour_curve(doc: &Document, id: ClipId) -> AutomationCurve {
    clip(doc, id).effects[0].keyframes["exposure_milli_stops"].clone()
}

fn colour_at(doc: &Document, id: ClipId, project_frame: i64) -> i64 {
    let clip = clip(doc, id);
    colour_curve(doc, id)
        .value_at(TimeCode(project_frame - clip.timeline_start.0))
        .expect("value")
}

/// AU4 §7 item A8: `MoveClip`, `SlipClip` and `ReplaceClip` leave the curve
/// alone.
#[test]
fn a_move_a_slip_and_a_replace_leave_the_clip_local_curves_verbatim() {
    let base = clip_with_envelope_and_colour_curve();

    // `MoveClip`: a clip-local curve rides along, byte-identical.
    let mut doc = base.clone();
    Operation::MoveClip {
        clip: ClipId(1),
        to_track: TrackId(1),
        to: TimeCode(90),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(envelope(&doc, ClipId(1)), envelope(&base, ClipId(1)));
    assert_eq!(
        envelope_at(&doc, ClipId(1), 120),
        envelope_at(&base, ClipId(1), 30)
    );

    // `SlipClip`: the slot and the duration are unchanged, so the curve is
    // untouched — and the material has slid out from under it (rule 21).
    let mut doc = base.clone();
    Operation::SlipClip {
        clip: ClipId(1),
        new_source_in: TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(envelope(&doc, ClipId(1)), envelope(&base, ClipId(1)));
    assert_eq!(
        colour_curve(&doc, ClipId(1)),
        colour_curve(&base, ClipId(1))
    );

    // `ReplaceClip`: verbatim onto different media, because the slot is
    // unchanged.
    let mut doc = base.clone();
    Operation::AddAsset {
        asset: asset(2, "asset-2", 300, fps()),
    }
    .apply(&mut doc)
    .unwrap();
    Operation::ReplaceClip {
        clip: ClipId(1),
        asset: AssetId(2),
        source: TimeCode(100)..TimeCode(160),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(envelope(&doc, ClipId(1)), envelope(&base, ClipId(1)));
    assert_eq!(
        colour_curve(&doc, ClipId(1)),
        colour_curve(&base, ClipId(1))
    );
}

/// AU4 §7 item A8: the split regression.
#[test]
fn a_split_no_longer_slides_the_right_halfs_clip_local_curves() {
    let base = clip_with_envelope_and_colour_curve();
    let mut doc = base.clone();
    Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(20),
    }
    .apply(&mut doc)
    .unwrap();
    let right = ClipId(2);
    assert_eq!(clip(&doc, right).timeline_start, TimeCode(20));

    // Both halves evaluate to the same values at the same **project** frames
    // as the original did — for the envelope *and* for the colour curve.
    for project_frame in 0..60 {
        let id = if project_frame < 20 { ClipId(1) } else { right };
        assert_eq!(
            envelope_at(&doc, id, project_frame),
            envelope_at(&base, ClipId(1), project_frame),
            "envelope at project frame {project_frame}"
        );
        assert_eq!(
            colour_at(&doc, id, project_frame),
            colour_at(&base, ClipId(1), project_frame),
            "colour curve at project frame {project_frame}"
        );
    }
    // The right half's own keying starts at local 0, which is the bug fixed.
    assert_eq!(envelope(&doc, right).keyframes[0].at, TimeCode::ZERO);
    doc.validate().unwrap();
}

/// AU4 §7 item A8: both trim edges, and the reject → clamp change.
#[test]
fn both_trim_edges_preserve_the_boundary_value_and_no_longer_fail() {
    let base = clip_with_envelope_and_colour_curve();

    // A right trim that shortens past a keyframe **succeeds** where it
    // previously returned `EffectKeyframeOutsideClip`.
    let mut doc = base.clone();
    Operation::TrimClip {
        clip: ClipId(1),
        new_source: TimeCode(0)..TimeCode(20),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(
        envelope_at(&doc, ClipId(1), 19),
        envelope_at(&base, ClipId(1), 19)
    );
    assert_eq!(
        colour_at(&doc, ClipId(1), 19),
        colour_at(&base, ClipId(1), 19)
    );
    doc.validate().unwrap();

    // A left trim that trims *in*: `delta_local > 0`, and the head is exact.
    let mut doc = base.clone();
    Operation::TrimClip {
        clip: ClipId(1),
        new_source: TimeCode(25)..TimeCode(60),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(clip(&doc, ClipId(1)).timeline_start, TimeCode(25));
    assert_eq!(
        envelope_at(&doc, ClipId(1), 25),
        envelope_at(&base, ClipId(1), 25)
    );
    assert_eq!(
        envelope_at(&doc, ClipId(1), 59),
        envelope_at(&base, ClipId(1), 59)
    );

    // A **written** over-running curve still fails: the invariant survives.
    let mut doc = base.clone();
    assert!(matches!(
        Operation::SetEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "exposure_milli_stops".to_owned(),
            curve: linear(&[(0, 0), (60, 4_720)]),
        }
        .apply(&mut doc),
        Err(OpError::EffectKeyframeOutsideClip {
            at: TimeCode(60),
            duration: TimeCode(60),
            ..
        })
    ));
}

/// AU4 §7 item A8: a left slide and a left roll reach rule 13's negative
/// branch.
#[test]
fn a_slide_and_a_roll_rebase_only_the_neighbour_whose_slot_moved() {
    let mut base = document_with_three_clips();
    let ramp = linear(&[(0, 0), (59, -590)]);
    for id in [ClipId(1), ClipId(2), ClipId(3)] {
        set_envelope(&mut base, id, &ramp);
    }

    // A **left** slide: the middle is untouched, the left neighbour shortens
    // on its own right edge, and the right neighbour's `delta_local` is
    // negative — nothing is inserted on its left and the exposed head is flat
    // at the first key's value.
    let mut doc = base.clone();
    Operation::SlideClip {
        clip: ClipId(2),
        to: TimeCode(45),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(envelope(&doc, ClipId(2)), ramp, "the middle is untouched");
    assert_eq!(
        envelope_at(&doc, ClipId(1), 44),
        envelope_at(&base, ClipId(1), 44)
    );
    let right = envelope(&doc, ClipId(3));
    assert_eq!(right.keyframes[0].at, TimeCode(15));
    for local in 0..15 {
        assert_eq!(right.value_at(TimeCode(local)), Some(0));
    }
    doc.validate().unwrap();

    // A **left** roll: the same negative branch on the right clip.
    let mut doc = base.clone();
    Operation::RollEdit {
        left_clip: ClipId(1),
        right_clip: ClipId(2),
        to: TimeCode(40),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(
        envelope_at(&doc, ClipId(1), 39),
        envelope_at(&base, ClipId(1), 39)
    );
    let right = envelope(&doc, ClipId(2));
    assert_eq!(right.keyframes[0].at, TimeCode(20));
    for local in 0..20 {
        assert_eq!(right.value_at(TimeCode(local)), Some(0));
    }

    // A **right** roll shortens the right clip's head: `delta_local > 0`.
    let mut doc = base.clone();
    Operation::RollEdit {
        left_clip: ClipId(1),
        right_clip: ClipId(2),
        to: TimeCode(75),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(
        envelope_at(&doc, ClipId(2), 75),
        envelope_at(&base, ClipId(2), 75)
    );
    assert_eq!(
        envelope_at(&doc, ClipId(1), 74),
        envelope_at(&base, ClipId(1), 74)
    );
}

/// AU4 §7 item A8: rule 25's destructive speed increase.
#[test]
fn a_speed_increase_drops_the_keys_past_the_new_duration_and_only_undo_restores_them() {
    let mut doc = document_with_one_clip();
    let ramp = linear(&[(0, 0), (30, -300), (59, -590)]);
    set_envelope(&mut doc, ClipId(1), &ramp);
    let before = doc.clone();

    Operation::SetClipSpeed {
        clip: ClipId(1),
        speed_percent: 200,
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.duration, TimeCode(30));
    let halved = envelope(&doc, ClipId(1));
    assert_eq!(
        halved
            .keyframes
            .iter()
            .map(|key| key.at.0)
            .collect::<Vec<_>>(),
        vec![0, 29]
    );
    assert_eq!(halved.value_at(TimeCode(29)), ramp.value_at(TimeCode(29)));

    // Returning to 100 % lengthens the clip; rule 15 correctly inserts nothing
    // and the dropped keys are gone.
    Operation::SetClipSpeed {
        clip: ClipId(1),
        speed_percent: 100,
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.duration, TimeCode(60));
    assert_eq!(envelope(&doc, ClipId(1)), halved);
    assert_ne!(envelope(&doc, ClipId(1)), ramp);

    // Only undo — here, the snapshot the journal keeps — restores them.
    assert_eq!(envelope(&before, ClipId(1)), ramp);
}

/// AU4 §7 item A8: the two ripples.
#[test]
fn ripple_delete_and_ripple_insert_shift_track_automation_and_never_a_bus_curve() {
    let mut base = document_with_three_clips();
    // One key every ten frames across the whole 180-frame project.
    let ride = linear(&[(0, 0), (60, -100), (70, -200), (120, -300), (179, -400)]);
    set_track_curve(&mut base, TrackId(1), "gain_tenth_db", &ride);
    Operation::UpsertAudioBus {
        bus: AudioBus {
            id: AudioBusId(1),
            name: "Music".to_owned(),
            tracks: vec![TrackId(1)],
            gain_tenth_db: 0,
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: Some(linear(&[(0, 0), (100, -50)])),
        },
    }
    .apply(&mut base)
    .unwrap();
    Operation::SetAudioMaster {
        master: AudioMaster {
            gain_tenth_db: 0,
            gain_curve: Some(linear(&[(0, 0), (100, -50)])),
            effects: Vec::new(),
        },
    }
    .apply(&mut base)
    .unwrap();

    // Ripple delete clip 2: `start = 60`, `ripple_point = 120`.
    let mut doc = base.clone();
    Operation::RippleDeleteClip { clip: ClipId(2) }
        .apply(&mut doc)
        .unwrap();
    let after = track_entry(&doc, TrackId(1))
        .unwrap()
        .gain_curve
        .clone()
        .unwrap();
    assert_eq!(
        after
            .keyframes
            .iter()
            .map(|key| (key.at.0, key.value))
            .collect::<Vec<_>>(),
        vec![(0, 0), (59, -98), (60, -300), (119, -400)],
        "the start-1 boundary key, the dropped window, and the shifted ripple_point key"
    );
    assert_eq!(
        after.value_at(TimeCode(59)),
        ride.value_at(TimeCode(59)),
        "the audio before the cut is unchanged"
    );
    // Rule 24: no bus or master curve moves, byte-identical.
    assert_eq!(
        doc.audio_mix.buses[0].gain_curve,
        base.audio_mix.buses[0].gain_curve
    );
    assert_eq!(
        doc.audio_mix.master.gain_curve,
        base.audio_mix.master.gain_curve
    );

    // Ripple insert at exactly a key's frame: it shifts, and nothing is
    // inserted.
    let mut doc = base.clone();
    Operation::RippleInsertGap {
        track: TrackId(1),
        at: TimeCode(60),
        duration: TimeCode(15),
    }
    .apply(&mut doc)
    .unwrap();
    let after = track_entry(&doc, TrackId(1))
        .unwrap()
        .gain_curve
        .clone()
        .unwrap();
    assert_eq!(
        after
            .keyframes
            .iter()
            .map(|key| (key.at.0, key.value))
            .collect::<Vec<_>>(),
        vec![(0, 0), (75, -100), (85, -200), (135, -300), (194, -400)]
    );
    assert_eq!(after.keyframes.len(), ride.keyframes.len());
    assert_eq!(
        doc.audio_mix.buses[0].gain_curve,
        base.audio_mix.buses[0].gain_curve
    );
}

/// AU4 §7 item A8 (rule 20, AU4 §0 E3): a ripple delete whose window swallows
/// every key must not leave an empty curve, which `validate` rejects.
#[test]
fn a_ripple_delete_that_removes_every_key_keeps_one_constant() {
    let mut doc = document_with_three_clips();
    // Every key sits inside the first clip, which is what the ripple removes,
    // and `start` is 0 so no boundary key is inserted at `start - 1`.
    set_track_curve(
        &mut doc,
        TrackId(1),
        "gain_tenth_db",
        &linear(&[(0, -100), (30, -400), (59, -200)]),
    );
    Operation::RippleDeleteClip { clip: ClipId(1) }
        .apply(&mut doc)
        .unwrap();
    let after = track_entry(&doc, TrackId(1))
        .unwrap()
        .gain_curve
        .clone()
        .unwrap();
    after.validate().unwrap();
    assert_eq!(after.keyframes.len(), 1);
    assert_eq!(after.keyframes[0].at, TimeCode::ZERO);
    // What the material now at frame 0 was carrying before the cut.
    assert_eq!(after.keyframes[0].value, -200);
    doc.validate().unwrap();
}

/// AU4 §7 item A8 (rule 20, AU4 §0 E2): a ripple insert on a track that does
/// not carry the project's last clip must not fail because its automation
/// shifted past an unchanged project end.
#[test]
fn a_ripple_insert_that_does_not_lengthen_the_project_clamps_rather_than_failing() {
    let mut doc = document_with_three_clips();
    // A second track whose only clip ends early, so rippling it moves no
    // project boundary. It does not sync-lock, so the ripple set is just it.
    Operation::AddTrack {
        track: Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: false,
            clips: Vec::new(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    Operation::AddClip {
        track: TrackId(2),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap();
    for track in &mut doc.tracks {
        if track.id == TrackId(1) {
            track.sync_lock = false;
        }
    }
    assert_eq!(doc.duration, TimeCode(180));
    set_track_curve(
        &mut doc,
        TrackId(2),
        "gain_tenth_db",
        &linear(&[(0, 0), (179, -400)]),
    );

    Operation::RippleInsertGap {
        track: TrackId(2),
        at: TimeCode(0),
        duration: TimeCode(20),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.duration, TimeCode(180), "the project did not grow");
    let after = track_entry(&doc, TrackId(2))
        .unwrap()
        .gain_curve
        .clone()
        .unwrap();
    assert!(
        after.keyframes.iter().all(|key| key.at < TimeCode(180)),
        "the shifted keys are clamped into the unchanged project: {after:?}"
    );
    doc.validate().unwrap();
}

/// AU4 §7 item A8: delete, remove-track and remove-bus take the curve with the
/// owner.
#[test]
fn deleting_an_owner_takes_its_curve_with_it() {
    let mut doc = document_with_three_clips();
    set_envelope(&mut doc, ClipId(2), &linear(&[(0, 0), (30, -100)]));
    Operation::DeleteClip { clip: ClipId(2) }
        .apply(&mut doc)
        .unwrap();
    assert!(
        doc.tracks
            .iter()
            .flat_map(|track| &track.clips)
            .all(|clip| clip.audio_gain_curve.is_none())
    );

    let mut doc = document_with_three_clips();
    set_track_curve(&mut doc, TrackId(1), "gain_tenth_db", &linear(&[(0, -100)]));
    Operation::AddTrack {
        track: Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: false,
            clips: Vec::new(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    set_track_curve(&mut doc, TrackId(2), "pan_percent", &linear(&[(0, 40)]));
    Operation::RemoveTrack { track: TrackId(2) }
        .apply(&mut doc)
        .unwrap();
    assert!(track_entry(&doc, TrackId(2)).is_none());
    assert!(track_entry(&doc, TrackId(1)).is_some());

    Operation::UpsertAudioBus {
        bus: AudioBus {
            id: AudioBusId(1),
            name: "Music".to_owned(),
            tracks: vec![TrackId(1)],
            gain_tenth_db: 0,
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: Some(linear(&[(0, -100)])),
        },
    }
    .apply(&mut doc)
    .unwrap();
    Operation::RemoveAudioBus { bus: AudioBusId(1) }
        .apply(&mut doc)
        .unwrap();
    assert!(doc.audio_mix.buses.is_empty());
}

/// AU4 §7 item A8: the fade clauses of rule 26.1.
#[test]
fn a_split_divides_the_audio_fades_and_a_short_trim_clamps_them() {
    let mut doc = document_with_one_clip();
    // A fully-faded clip: 30 + 30 over a 60-frame clip. Today both halves keep
    // both fades and the split fails with `AudioFadesTooLong`.
    Operation::SetClipAudio {
        clip: ClipId(1),
        gain_tenth_db: 0,
        fade_in_frames: TimeCode(30),
        fade_out_frames: TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap();
    let mut split = doc.clone();
    Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(20),
    }
    .apply(&mut split)
    .unwrap();
    let left = clip(&split, ClipId(1));
    let right = clip(&split, ClipId(2));
    assert_eq!(
        left.audio_fade_out_frames,
        TimeCode::ZERO,
        "cut side zeroed"
    );
    assert_eq!(
        right.audio_fade_in_frames,
        TimeCode::ZERO,
        "cut side zeroed"
    );
    // The left half is only 20 frames long, so its fade-in clamps to 20.
    assert_eq!(left.audio_fade_in_frames, TimeCode(20));
    assert_eq!(right.audio_fade_out_frames, TimeCode(30));
    split.validate().unwrap();

    // A trim shorter than `fade_in + fade_out` clamps instead of raising
    // `AudioFadesTooLong`; the fade-out is reduced first.
    let mut trimmed = doc.clone();
    Operation::TrimClip {
        clip: ClipId(1),
        new_source: TimeCode(0)..TimeCode(40),
    }
    .apply(&mut trimmed)
    .unwrap();
    let clip = clip(&trimmed, ClipId(1));
    assert_eq!(clip.audio_fade_in_frames, TimeCode(30));
    assert_eq!(clip.audio_fade_out_frames, TimeCode(10));
    trimmed.validate().unwrap();

    // And *writing* a bad pair is still rejected.
    assert!(matches!(
        Operation::SetClipAudio {
            clip: ClipId(1),
            gain_tenth_db: 0,
            fade_in_frames: TimeCode(40),
            fade_out_frames: TimeCode(40),
        }
        .apply(&mut trimmed),
        Err(OpError::AudioFadesTooLong { .. })
    ));
}

/// AU4 §7 item A8: `RelinkAsset`.
#[test]
fn relinking_an_asset_keeps_every_bound_clips_curve_inside_its_duration() {
    let mut doc = clip_with_envelope_and_colour_curve();
    let before = doc.clone();
    let current = doc.asset(AssetId(1)).unwrap().clone();
    let fingerprint = MediaSourceFingerprint {
        content_sha256: Some("a".repeat(64)),
        byte_len: Some(4_096),
    };
    Operation::RelinkAsset {
        asset: AssetId(1),
        candidate: RelinkCandidate {
            path: std::path::PathBuf::from("relinked.mp4"),
            kind: current.kind,
            fps: current.fps,
            duration: current.duration,
            resolution: current.resolution,
            fingerprint,
        },
        allow_unverified_source: true,
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(
        doc.asset(AssetId(1)).unwrap().path,
        std::path::PathBuf::from("relinked.mp4")
    );
    // `relink_asset` rejects an fps, duration or resolution mismatch outright
    // (`RelinkMetadataMismatch`), so no bound clip's project duration can
    // change and every curve is left verbatim (AU4 §0 E4).
    assert_eq!(envelope(&doc, ClipId(1)), envelope(&before, ClipId(1)));
    assert_eq!(
        colour_curve(&doc, ClipId(1)),
        colour_curve(&before, ClipId(1))
    );
    doc.validate().unwrap();
}

// ---------------------------------------------------------------------------
// A13, A16, A20 — the core halves
// ---------------------------------------------------------------------------

/// AU4 §7 item A13: every new serialized default is omitted, so a curve-free
/// document is byte-identical to a pre-AU4 one.
#[test]
fn a_curve_free_document_carries_no_new_key_and_a_curve_bearing_one_carries_one_key_per_curve() {
    let mut doc = automated_project();
    set_envelope(&mut doc, ClipId(1), &linear(&[(0, 0), (30, -100)]));
    set_track_curve(&mut doc, TrackId(1), "pan_percent", &linear(&[(0, 30)]));
    let json = serde_json::to_string(&doc).unwrap();
    // Clip 1's envelope set here plus clip 2's from `automated_project`.
    assert_eq!(json.matches("\"audio_gain_curve\"").count(), 2);
    assert_eq!(json.matches("\"pan_curve\"").count(), 1);
    // The track ride, the bus fader and the master fader.
    assert_eq!(json.matches("\"gain_curve\"").count(), 3);
}

#[derive(Default)]
struct CountingPlayback {
    documents: AtomicUsize,
}

impl Playback for CountingPlayback {
    fn set_document(&self, _doc: Arc<Document>) {
        self.documents.fetch_add(1, Ordering::SeqCst);
    }
    fn request_frame(&self, _t: TimeCode) {}
    fn frames(&self) -> Receiver<(TimeCode, FrameTexture)> {
        crossbeam_channel::unbounded().1
    }
    fn events(&self) -> Receiver<MediaEvent> {
        crossbeam_channel::unbounded().1
    }
    fn play(&self, _from: TimeCode) {}
    fn pause(&self) {}
    fn seek(&self, _to: TimeCode) {}
    fn position(&self) -> TimeCode {
        TimeCode::ZERO
    }
    fn output_peaks(&self) -> [f32; 2] {
        [0.0, 0.0]
    }
}

/// AU4 §7 item A16: `Playback::update_clip_shaping` is **defaulted**, so no
/// test double changes; the default is `set_document`.
#[test]
fn playback_update_clip_shaping_defaults_to_set_document_on_a_counting_double() {
    let playback = CountingPlayback::default();
    let doc = Arc::new(document_with_one_clip());
    assert_eq!(playback.documents.load(Ordering::SeqCst), 0);
    playback.update_clip_shaping(Arc::clone(&doc));
    assert_eq!(playback.documents.load(Ordering::SeqCst), 1);
    // Beside the existing defaulted `update_audio_mix`, which behaves the same.
    playback.update_audio_mix(doc);
    assert_eq!(playback.documents.load(Ordering::SeqCst), 2);
    // And the AU3 defaults are untouched.
    assert_eq!(playback.mix_peaks(), kinewright_core::MixPeaks::default());
}

/// AU4 §7 item A20: QC is unchanged.
#[test]
fn a_curve_bearing_document_produces_the_same_qa_issue_set_as_one_without() {
    let plain = automated_project();
    let mut curved = plain.clone();
    set_envelope(&mut curved, ClipId(1), &linear(&[(0, 0), (30, -600)]));
    set_envelope(&mut curved, ClipId(2), &linear(&[(0, -600), (30, 0)]));
    set_track_curve(
        &mut curved,
        TrackId(1),
        "pan_percent",
        &linear(&[(0, -100)]),
    );

    let codes = |doc: &Document| {
        qa_document(doc)
            .issues
            .iter()
            .map(|issue| (issue.code.clone(), issue.severity, issue.message.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(codes(&curved), codes(&plain));

    // Including on a retimed clip, whose envelope rule 25 deliberately leaves
    // in place: `retimed_audio_muted` keeps its meaning.
    let mut retimed = plain.clone();
    Operation::SetClipSpeed {
        clip: ClipId(2),
        speed_percent: 200,
    }
    .apply(&mut retimed)
    .unwrap();
    let mut retimed_curved = retimed.clone();
    set_envelope(&mut retimed_curved, ClipId(2), &linear(&[(0, -200)]));
    assert_eq!(codes(&retimed_curved), codes(&retimed));
}

/// Assert that `keys` appear in the given order in `value`'s serialization.
///
/// `to_string` writes struct fields in declaration order; `to_value` sorts
/// them, so it cannot be used here.
fn assert_wire_order<T: serde::Serialize>(value: &T, keys: &[&str]) {
    let json = serde_json::to_string(value).unwrap();
    let mut previous = 0;
    for key in keys {
        let needle = format!("\"{key}\"");
        let at = json
            .find(&needle)
            .unwrap_or_else(|| panic!("{needle} is on the wire: {json}"));
        assert!(at >= previous, "{needle} is out of order in {json}");
        previous = at;
    }
}

/// AU4 §7 item A1 (rule 3): field order is wire order.
#[test]
fn each_new_field_sits_at_its_declared_wire_position() {
    let mut doc = document_with_one_clip();
    let curve = linear(&[(0, 0), (30, -100)]);
    set_envelope(&mut doc, ClipId(1), &curve);
    set_track_curve(&mut doc, TrackId(1), "gain_tenth_db", &curve);
    set_track_curve(&mut doc, TrackId(1), "pan_percent", &linear(&[(0, 20)]));
    Operation::SetClipAudio {
        clip: ClipId(1),
        gain_tenth_db: -20,
        fade_in_frames: TimeCode(5),
        fade_out_frames: TimeCode(6),
    }
    .apply(&mut doc)
    .unwrap();
    Operation::SetTrackMix {
        track: TrackId(1),
        gain_tenth_db: -30,
        pan_percent: 10,
        mute: true,
        solo: false,
    }
    .apply(&mut doc)
    .unwrap();
    Operation::SetClipSpeed {
        clip: ClipId(1),
        speed_percent: 50,
    }
    .apply(&mut doc)
    .unwrap();
    doc.audio_mix.buses.push(AudioBus {
        id: AudioBusId(1),
        name: "Music".to_owned(),
        tracks: vec![TrackId(1)],
        gain_tenth_db: -15,
        effects: Vec::new(),
        ducking_sidechain_tracks: Vec::new(),
        gain_curve: Some(curve.clone()),
    });
    doc.audio_mix.master.gain_tenth_db = -5;
    doc.audio_mix.master.gain_curve = Some(curve);
    doc.validate().unwrap();

    // Each owner is checked on its own serialization, because the document's
    // own field order interleaves the four.
    assert_wire_order(
        clip(&doc, ClipId(1)),
        &[
            "audio_fade_in_frames",
            "audio_fade_out_frames",
            "audio_gain_curve",
            "speed_percent",
        ],
    );
    assert_wire_order(
        track_entry(&doc, TrackId(1)).unwrap(),
        &[
            "gain_tenth_db",
            "pan_percent",
            "gain_curve",
            "pan_curve",
            "mute",
        ],
    );
    assert_wire_order(&doc.audio_mix.buses[0], &["gain_tenth_db", "gain_curve"]);
    assert_wire_order(&doc.audio_mix.master, &["gain_tenth_db", "gain_curve"]);
}
