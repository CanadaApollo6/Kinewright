struct Item12Tone {
    path: std::path::PathBuf,
    bytes: Vec<u8>,
}
impl Item12Tone {
    fn new() -> Self {
        use std::io::Write as _;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut bytes = Vec::with_capacity(44 + 192_000 * 2);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36_u32 + 384_000).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&384_000_u32.to_le_bytes());
        for frame in 0..192_000 {
            let sample: i16 = if frame % 48 < 24 { 8000 } else { -8000 };
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "dq-item12-tone-{}-{sequence}.wav",
            std::process::id()
        ));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        Self { path, bytes }
    }
}
impl Drop for Item12Tone {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).unwrap();
    }
}
struct Item12Bindings {
    target: Arc<BoundDialogue>,
    sync: Option<Arc<BoundDialogue>>,
}
impl DialogueMeasurements for Item12Bindings {
    fn audio_stream(&self, id: &MediaIdentity) -> Result<u32, Refusal> {
        if id == self.target.identity() || self.sync.as_ref().is_some_and(|s| id == s.identity()) {
            Ok(0)
        } else {
            Err(Refusal::NotMeasured)
        }
    }
    fn get(&self, id: &MediaIdentity, _: u32) -> Result<Arc<BoundDialogue>, Refusal> {
        if id == self.target.identity() {
            Ok(Arc::clone(&self.target))
        } else {
            self.sync
                .as_ref()
                .filter(|s| id == s.identity())
                .map(Arc::clone)
                .ok_or(Refusal::NotMeasured)
        }
    }
}
fn item12_sync_document(
    tone: &Item12Tone,
    project: Rational,
    sync_rate: Rational,
    sync_frames: i64,
) -> Document {
    let mut doc = document(project, project, 0, i64::from(project.numerator()) * 4);
    // The sync recording is a separate asset and track with its own editable state.
    let mut asset = doc.media_pool[0].clone();
    asset.id = AssetId(2);
    asset.fps = sync_rate;
    asset.duration = TimeCode(sync_frames);
    asset.path = tone.path.clone();
    asset.source_fingerprint.content_sha256 = Some(kinewright_media::sha256_bytes(&tone.bytes));
    asset.source_fingerprint.byte_len = Some(u64::try_from(tone.bytes.len()).unwrap());
    doc.media_pool.push(asset);
    let mut track = doc.tracks[0].clone();
    track.id = TrackId(2);
    track.clips[0].id = ClipId(2);
    track.clips[0].asset = AssetId(2);
    track.clips[0].source_range = TimeCode(0)..TimeCode(sync_frames);
    doc.tracks.push(track);
    doc.validate().unwrap();
    doc
}
fn item12_service(doc: Document, target: Arc<BoundDialogue>, measured: bool) -> KinewrightMcp {
    let sync = measured.then(|| {
        Arc::new(
            BoundDialogue::new(
                MediaIdentity::of(doc.asset(AssetId(2)).unwrap()).unwrap(),
                0,
                "literal-1khz-square".into(),
                SoundProfile::from_hops(
                    10_000,
                    4_000_000,
                    (0..400).map(|_| Hop {
                        rms_dbfs_hundredths: -1225,
                        peak_dbfs_hundredths: -1225,
                        // Fully sounding, voice-positive, but untranscribed.
                        // A measured speech population calibrates this fake value.
                        voice: true,
                    }),
                )
                .unwrap(),
                vec![],
                Settings::default(),
            )
            .unwrap(),
        )
    });
    let s = service(doc, Arc::clone(&target));
    *s.dialogue_measurements.write().unwrap() = Some(Arc::new(Item12Bindings { target, sync }));
    s
}
fn item12_assert_sync_zero_change(before: &Document, after: &Document, reply: &CallToolResult) {
    let project =
        |frame| kinewright_core::ExactMicros::frames(TimeCode(frame), before.fps).unwrap();
    let removal = &content(reply)["removals"][0]["cut"];
    let start = project(removal[0].as_i64().unwrap());
    let end = project(removal[1].as_i64().unwrap());
    let original = dq::spans(before)
        .unwrap()
        .into_iter()
        .find(|s| s.track == TrackId(2))
        .unwrap();
    let mut kept: Vec<_> = dq::spans(after)
        .unwrap()
        .into_iter()
        .filter(|s| s.track == TrackId(2))
        .collect();
    kept.sort_by_key(|s| s.project.start);
    assert_eq!(kept.len(), 2, "DQ_TOOLS_UNMEASURED_SYNC_ZERO_CHANGE");
    assert_eq!(
        [kept[0].source.start, kept[0].source.end],
        [original.source.start, start],
        "DQ_TOOLS_UNMEASURED_SYNC_ZERO_CHANGE"
    );
    assert_eq!(
        [kept[1].source.start, kept[1].source.end],
        [end, original.source.end],
        "DQ_TOOLS_UNMEASURED_SYNC_ZERO_CHANGE"
    );
    assert_eq!(kept[0].source.padding, kinewright_core::ExactMicros::ZERO);
    assert_eq!(kept[1].source.padding, original.source.padding);
    assert!(
        content(reply)["extent_changes"]
            .get("unmeasured_extents")
            .is_none()
    );
    assert!(
        content(reply)["extent_changes"]
            .get("largest_unmeasured_us")
            .is_none()
    );
}
#[test]
fn dq_tools_item12_unmeasured_sync_sounding_loss_is_refused() {
    let tone = Item12Tone::new();
    assert_eq!(tone.bytes.len(), 44 + 192_000 * 2);
    let old_loss = &tone.bytes[44 + 191_040 * 2..];
    assert_eq!(old_loss.len() / 2, 960);
    assert!(
        old_loss
            .as_chunks::<2>()
            .0
            .iter()
            .all(|b| i16::from_le_bytes(*b).unsigned_abs() == 8000)
    );
    for overrule in [false, true] {
        for measured in [false, true] {
            let v = tail_value(false, true);
            let doc = item12_sync_document(
                &tone,
                Rational::new(24, 1).unwrap(),
                Rational::new(25, 1).unwrap(),
                100,
            );
            let s = item12_service(doc, Arc::clone(&v), measured);
            let before = s.snapshot().unwrap();
            let history = s.core.export_history_identified().unwrap();
            let mut removal = words(&v, 1);
            if overrule {
                removal["overrule"] = json!([1_100_000, 1_300_000]);
            }
            let reply = cut(&s, args(vec![removal]), true);
            println!(
                "DQ_ITEM12_SYNC measured={measured} overrule={overrule} {}",
                content(&reply)
            );
            assert_refused_unchanged(
                &s,
                &before,
                &history,
                &reply,
                "DQ_TOOLS_UNMEASURED_SYNC_PRESERVED",
            );
            let reason = &content(&reply)["removals"][0];
            assert_eq!(reason["clip"], 2);
            if measured {
                assert_eq!(reason["code"], "unselected_audio");
                assert!(reason["why"].as_str().unwrap().contains("not proven quiet"));
            } else {
                assert_eq!(reason["code"], "unmeasured_extent");
                assert!(reason["changed_micros"].as_u64().unwrap() > 0);
                assert!(reason["why"].as_str().unwrap().contains("not measured"));
            }
        }
    }
}
#[test]
fn dq_tools_item12_equal_rate_unmeasured_sync_still_cuts_exactly() {
    let tone = Item12Tone::new();
    let fps = Rational::new(24, 1).unwrap();
    let doc = item12_sync_document(&tone, fps, fps, 96);
    let v = tail_value(false, true);
    let s = item12_service(doc, Arc::clone(&v), false);
    let before = s.snapshot().unwrap();
    let r = cut(&s, args(vec![words(&v, 1)]), true);
    ok(&r);
    item12_assert_sync_zero_change(&before.1, &s.snapshot().unwrap().1, &r);
    s.core.request(Command::Undo).unwrap();
    assert_eq!(s.snapshot().unwrap().1, before.1);
}
#[test]
fn dq_tools_item12_unproven_sync_origin_refuses_changed_extents() {
    let tone = Item12Tone::new();
    let mut doc = item12_sync_document(
        &tone,
        Rational::new(24, 1).unwrap(),
        Rational::new(25, 1).unwrap(),
        100,
    );
    doc.tracks[1].clips[0].enabled_curve = Some(kinewright_core::AutomationCurve {
        keyframes: [(0, 0), (1, 1)]
            .into_iter()
            .map(|(at, value)| kinewright_core::Keyframe {
                at: TimeCode(at),
                value,
                interpolation: kinewright_core::KeyframeInterpolation::Hold,
                tangent_in: 0,
                tangent_out: 0,
            })
            .collect(),
    });
    let v = tail_value(false, true);
    let s = item12_service(doc, Arc::clone(&v), true);
    let before = s.snapshot().unwrap();
    let history = s.core.export_history_identified().unwrap();
    assert!(
        dq::spans(&before.1)
            .unwrap()
            .iter()
            .any(|p| p.track == TrackId(2) && !p.proven)
    );
    let r = cut(&s, args(vec![words(&v, 1)]), true);
    println!("DQ_ITEM12_UNPROVEN {}", content(&r));
    assert_refused_unchanged(
        &s,
        &before,
        &history,
        &r,
        "DQ_TOOLS_UNPROVEN_SYNC_PRESERVED",
    );
    let reason = &content(&r)["removals"][0];
    assert_eq!(reason["code"], "unproven_extent");
    assert_eq!(reason["clip"], 2);
    assert!(reason["changed_micros"].as_u64().unwrap() > 0);
    assert!(reason["why"].as_str().unwrap().contains("origin unproven"));
}
#[test]
fn dq_tools_item12_unmeasured_sync_growth_is_refused() {
    let tone = Item12Tone::new();
    let doc = item12_sync_document(
        &tone,
        Rational::new(30, 1).unwrap(),
        Rational::new(30_000, 1001).unwrap(),
        120,
    );
    let v = ordinary();
    let s = item12_service(doc, Arc::clone(&v), false);
    let before = s.snapshot().unwrap();
    let history = s.core.export_history_identified().unwrap();
    let r = cut(
        &s,
        args(vec![
            json!({"kind":"words","first":address(&v,1),"last":address(&v,1),"overrule":[650_000,1_900_000]}),
        ]),
        true,
    );
    println!("DQ_ITEM12_GROWTH {}", content(&r));
    assert_refused_unchanged(
        &s,
        &before,
        &history,
        &r,
        "DQ_TOOLS_UNMEASURED_GROWTH_PRESERVED",
    );
    assert_eq!(content(&r)["removals"][0]["code"], "unmeasured_extent");
    assert_eq!(content(&r)["removals"][0]["clip"], 2);
    assert!(
        content(&r)["removals"][0]["changed_micros"]
            .as_u64()
            .unwrap()
            > 0
    );
}
