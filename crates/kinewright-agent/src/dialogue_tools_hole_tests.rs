#[test]
fn dq_tools_holes_report_measured_basis_and_ignore_word_ends() {
    for end_micros in [180_000, 900_000] {
        let v = value(
            vec![Word {
                text: "known".into(),
                start_micros: 100_000,
                end_micros,
            }],
            2_000_000,
            |t| {
                let voice = (100_000..900_000).contains(&t) || (1_500_000..1_800_000).contains(&t);
                Hop {
                    rms_dbfs_hundredths: if voice { -2_000 } else { -8_000 },
                    peak_dbfs_hundredths: if voice { -1_400 } else { -7_400 },
                    voice,
                }
            },
        );
        let fps = Rational::new(100, 1).unwrap();
        let s = service(document(fps, fps, 0, 200), v);
        let r = invoke(
            &s,
            "check_dialogue_cut",
            json!({"track_id":1,"detail":"all"}),
        );
        ok(&r);
        let report = &content(&r)["check"]["segments"][0]["report"];
        assert_eq!(
            report["hole_basis"],
            json!({"kind":"measured_voice_without_nearby_word_start","after_word_start_us":1_000_000}),
            "DQ_TOOLS_MEASURED_HOLE_BASIS"
        );
        assert_eq!(report["total"]["holes"], 1);
        assert_eq!(report["total"]["sounding_holes"], 1);
        let holes: Vec<_> = report["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|h| h["kind"] == "hole")
            .collect();
        assert_eq!(holes.len(), 1);
        assert_eq!(
            holes[0]["location"]["source_us"],
            json!([1_500_000, 1_800_000])
        );
        assert_eq!(holes[0]["threshold_cdb"], -4_001);
    }
}
