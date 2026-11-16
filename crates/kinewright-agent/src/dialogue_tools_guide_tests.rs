#[test]
fn dq_guide_is_bounded_and_pinned() {
    assert!(dq::GUIDE.len() <= 1500);
    assert_eq!(
        kinewright_media::sha256_bytes(dq::GUIDE.as_bytes()),
        "73e3bc9715254c900a709fde405e1f22a620bd33c406a7e8a158358557f201aa"
    );
}
#[test]
fn dq_guide_arrives_once_with_the_cut_schema() {
    let (mut s, _) = fixture();
    s.host_scoped = true;
    let mut r = s
        .call_exposed_inner(CallToolRequestParams::new("get_capability").with_arguments(
            serde_json::Map::from_iter([("name".into(), json!("cut_dialogue"))]),
        ))
        .unwrap();
    ok(&r);
    assert_eq!(content(&r)["guide"], dq::GUIDE);
    assert_eq!(content(&r)["capability"]["name"], "cut_dialogue");
    let with = serde_json::to_vec(&r).unwrap().len();
    r.structured_content
        .as_mut()
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("guide");
    let without = serde_json::to_vec(&r).unwrap().len();
    println!(
        "DQ_GUIDE_BYTES text={} lookup_extra={} lookup_total={with}",
        dq::GUIDE.len(),
        with - without
    );
    let other = s
        .call_exposed_inner(CallToolRequestParams::new("get_capability").with_arguments(
            serde_json::Map::from_iter([("name".into(), json!("check_dialogue_cut"))]),
        ))
        .unwrap();
    ok(&other);
    assert!(content(&other).get("guide").is_none());
    let batch = s
        .call_exposed_inner(CallToolRequestParams::new("get_capability").with_arguments(
            serde_json::Map::from_iter([(
                "names".into(),
                json!(["cut_dialogue", "check_dialogue_cut"]),
            )]),
        ))
        .unwrap();
    ok(&batch);
    assert_eq!(
        content(&batch)["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c.get("guide").is_some())
            .count(),
        1
    );
}
#[test]
fn dq_guide_is_available_before_measurement_and_absent_from_catalog() {
    let (s, _) = fixture();
    *s.dialogue_measurements.write().unwrap() = None;
    let r = s
        .call_exposed_inner(CallToolRequestParams::new("get_capability").with_arguments(
            serde_json::Map::from_iter([("name".into(), json!("cut_dialogue"))]),
        ))
        .unwrap();
    ok(&r);
    assert_eq!(content(&r)["guide"], dq::GUIDE);
    assert!(
        !serde_json::to_string(&KinewrightMcp::capability_tools().unwrap())
            .unwrap()
            .contains("Keep the speaker's meaning")
    );
}
