use kinewright_core::LossCode;
use kinewright_interchange::{
    MediaTable, SiteQuery, fcpxml, ir::ParseOptions, lower, lower_with_sites, otio,
};

#[test]
fn written_zero_audio_never_borrows_time_from_its_neighbours() {
    let bytes = include_bytes!("fixtures/resolve-shapes/zero-duration.otio");
    let timeline = otio::parse(bytes, "zero-duration.otio", ParseOptions::default()).unwrap();
    let result = lower_with_sites(
        &timeline,
        &MediaTable::default(),
        Some(SiteQuery {
            code: LossCode::ClipTooShort,
            after: 0,
        }),
    )
    .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    for track in value["tracks"]["children"].as_array_mut().unwrap() {
        track["children"].as_array_mut().unwrap().remove(1);
    }
    let without = otio::parse(
        &serde_json::to_vec(&value).unwrap(),
        "zero-duration.otio",
        ParseOptions::default(),
    )
    .unwrap();
    let (mut expected, _) = lower(&without, &MediaTable::default()).unwrap();
    let mut actual = result.document;
    expected.imported_from.clear();
    actual.imported_from.clear();
    assert_eq!(
        actual.tracks[0].clips.len(),
        2,
        "the short predecessor must not leave a third clip to fill its lost time"
    );
    assert_eq!(result.sites.sites.len(), 3);
    assert!(
        result
            .sites
            .sites
            .iter()
            .all(|site| site.clip.is_none() && site.source.ends_with("/children/1"))
    );
    assert_eq!(actual, expected);
}

#[test]
fn written_zero_fcpxml_clip_does_not_advance_the_spine() {
    let bytes = include_str!("fixtures/resolve-shapes/zero-duration.fcpxml");
    let with = fcpxml::parse(
        bytes.as_bytes(),
        "zero-duration.fcpxml",
        ParseOptions::default(),
    )
    .unwrap();
    let without = bytes.replace(
        "    <asset-clip ref=\"z\" offset=\"5s\" duration=\"0s\"/>\n",
        "",
    );
    let without = fcpxml::parse(
        without.as_bytes(),
        "zero-duration.fcpxml",
        ParseOptions::default(),
    )
    .unwrap();
    let (mut actual, record) = lower(&with, &MediaTable::default()).unwrap();
    let (mut expected, _) = lower(&without, &MediaTable::default()).unwrap();
    actual.imported_from.clear();
    expected.imported_from.clear();
    assert_eq!(
        actual.duration.0, 48,
        "the written zero must not move the implicit offset"
    );
    assert_eq!(actual, expected);
    let row = record
        .rows
        .iter()
        .find(|row| row.code == LossCode::ClipTooShort)
        .unwrap();
    assert_eq!(row.count, 1);
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 256, failure_persistence: None, ..Default::default()
    })]

    #[test]
    fn erasing_written_zero_items_keeps_the_same_timeline(
        spans in proptest::collection::vec((1_u16..80, proptest::bool::ANY, proptest::bool::ANY), 2..12),
        nested in proptest::bool::ANY,
    ) {
        use serde_json::{Value, json};

        let clip = |locator: &str, span| json!({
            "OTIO_SCHEMA":"Clip.1",
            "source_range":{"start_time":{"rate":24,"value":0},"duration":{"rate":24,"value":span}},
            "media_reference":{"OTIO_SCHEMA":"ExternalReference.1","target_url":locator},
        });
        let mut with = Vec::new();
        let mut without = Vec::new();
        let mut zero_count = 0;
        for (index, (span, fractional, add_gap)) in spans.into_iter().enumerate() {
            let ordinary = clip("ordinary.mov", f64::from(span));
            with.push(ordinary.clone());
            without.push(ordinary);
            if add_gap {
                let gap = json!({"OTIO_SCHEMA":"Gap.1",
                    "source_range":{"start_time":{"rate":24,"value":0},"duration":{"rate":24,"value":3}}});
                with.push(gap.clone());
                without.push(gap);
            }
            // A positive subframe stays in both inputs and still follows ruling 6.
            let positive = clip("ordinary.mov", 0.25);
            with.push(positive.clone());
            without.push(positive);
            with.push(clip(&format!("zero-{index}.mov"), if fractional { 0.000_000_5 } else { 0.0 }));
            zero_count += 1;
        }
        let read = |children: Vec<Value>| {
            let track = |children| json!({"OTIO_SCHEMA":"Track.1","kind":"Video","children":children});
            let stack = |track| json!({"OTIO_SCHEMA":"Stack.1","children":[track]});
            let inner = track(children);
            let tracks = if nested { stack(track(vec![stack(inner)])) } else { stack(inner) };
            let bytes = serde_json::to_vec(&json!({
                "OTIO_SCHEMA":"Timeline.1", "global_start_time":{"rate":24,"value":0}, "tracks":tracks
            })).unwrap();
            let parsed = otio::parse(&bytes, "zero-property.otio", ParseOptions::default()).unwrap();
            let (mut document, record) = lower(&parsed, &MediaTable::default()).unwrap();
            document.imported_from.clear();
            (document, record)
        };
        let (actual, actual_record) = read(with);
        let (expected, expected_record) = read(without);
        proptest::prop_assert!(actual.media_pool.iter().all(|asset| asset.path == std::path::Path::new("ordinary.mov")));
        proptest::prop_assert_eq!(actual, expected);
        let short = |record: &kinewright_core::ImportRecord| record.rows.iter()
            .find(|row| row.code == LossCode::ClipTooShort).map_or(0, |row| row.count);
        proptest::prop_assert_eq!(short(&actual_record), short(&expected_record) + zero_count);
        proptest::prop_assert_eq!(actual_record.dropped, expected_record.dropped + zero_count);
    }
}

#[test]
fn differing_otio_reference_ranges_share_the_union_and_keep_their_in_points() {
    use kinewright_core::TimeCode;
    let bytes = include_bytes!("fixtures/resolve-shapes/shared-source-ranges.otio");
    let timeline =
        otio::parse(bytes, "shared-source-ranges.otio", ParseOptions::default()).unwrap();
    let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
    assert_eq!(document.media_pool.len(), 1);
    // [84260, 84297) and [36986, 37066) give origin 36986. The first written
    // clip extends the end to 84300, so the offline asset has 47314 frames.
    assert_eq!(document.media_pool[0].duration, TimeCode(47_314));
    let clips = &document.tracks[0].clips;
    assert_eq!(clips.len(), 3);
    // The 0.85x in-points round from 37008.099992 - 36986 = 22.099992
    // and 37036.149992000006 - 36986 = 50.149992000006 on the 24 fps grid.
    assert_eq!(clips[0].source_range.start, TimeCode(47_274));
    assert_eq!(clips[1].source_range.start, TimeCode(22));
    assert_eq!(clips[2].source_range.start, TimeCode(50));
    assert_eq!(document.duration, TimeCode(78));
    let row = record
        .rows
        .iter()
        .find(|row| row.code == LossCode::MediaRangesMerged)
        .unwrap();
    assert_eq!(row.count, 3);
    assert_eq!(row.fidelity, kinewright_core::Fidelity::Approximated);

    let online = MediaTable {
        entries: vec![kinewright_interchange::Availability::Probed(Box::new(
            kinewright_interchange::ProbedMedia {
                path: "shared.wav".into(),
                kind: kinewright_core::MediaKind::Audio,
                fps: kinewright_core::Rational::new(24, 1).unwrap(),
                duration: TimeCode(50_000),
                resolution: None,
                source_fingerprint: kinewright_core::MediaSourceFingerprint {
                    content_sha256: Some("a".repeat(64)),
                    byte_len: Some(640),
                },
                color_description: kinewright_core::ColorDescription::default(),
                relinked_by_name: false,
            },
        ))],
    };
    let (verified, verified_record) = lower(&timeline, &online).unwrap();
    assert_eq!(verified.media_pool[0].duration, TimeCode(50_000));
    assert_eq!(verified.tracks[0].clips, document.tracks[0].clips);
    assert!(
        !verified_record
            .rows
            .iter()
            .any(|row| row.code == LossCode::MediaRangesMerged)
    );
}

#[test]
fn fcpxml_resources_sharing_a_locator_keep_distinct_origins() {
    use kinewright_core::TimeCode;
    let bytes = br#"<fcpxml version="1.13">
      <resources><format id="f" frameDuration="1/24s"/>
        <asset id="a" start="100/24s" duration="20/24s" format="f" hasVideo="1">
          <media-rep kind="original-media" src="shared.mov"/>
        </asset>
        <asset id="b" start="200/24s" duration="40/24s" format="f" hasVideo="1">
          <media-rep kind="original-media" src="shared.mov"/>
        </asset>
      </resources>
      <project><sequence format="f"><spine>
        <asset-clip ref="a" start="103/24s" duration="5/24s"/>
        <asset-clip ref="b" start="207/24s" duration="5/24s"/>
      </spine></sequence></project>
    </fcpxml>"#;
    let timeline = fcpxml::parse(bytes, "shared.fcpxml", ParseOptions::default()).unwrap();
    let (document, _) = lower(&timeline, &MediaTable::default()).unwrap();
    let clips = &document.tracks[0].clips;
    assert_eq!(document.media_pool.len(), 2);
    assert_eq!(clips[0].source_range, TimeCode(3)..TimeCode(8));
    assert_eq!(clips[1].source_range, TimeCode(7)..TimeCode(12));
    assert_ne!(clips[0].asset, clips[1].asset);
}
