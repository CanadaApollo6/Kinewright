// Temporary unit probe included from operation_history/stored_codec_tests.rs.
// No product code or role limits are changed by this probe.
fn sizing_probe_owner(role: Role, spare_clips: usize) -> OwnerHistory {
    OwnerHistory::new(
        &HistoryPool::new(PoolLimits::default()),
        role,
        role.policy(),
        &origin(1),
        || {
            let mut doc = document();
            doc.tracks[0].clips.reserve(spare_clips);
            Ok(doc)
        },
    )
    .unwrap()
}
fn sizing_probe_record(owner: &OwnerHistory) -> Vec<u8> {
    let snapshot = owner.capture().unwrap();
    let reference = |doc: &kinewright_core::Document| {
        let bytes = serde_json::to_vec(doc).unwrap();
        ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::Document,
        }
    };
    let record = snapshot
        .stored_record(
            reference(&snapshot.data.history.baseline),
            reference(&snapshot.data.history.document),
            &[],
        )
        .unwrap();
    serde_json::to_vec(&record).unwrap()
}
fn sizing_probe_discarded_delta(count: usize) -> (OwnerHistory, kinewright_core::undo::Charge) {
    let mut owner = sizing_probe_owner(Role::Candidate, 0);
    let operations = (0..count)
        .map(|i| Operation::SetTrackSyncLock {
            track: TrackId(81),
            locked: i % 2 != 0,
        })
        .collect();
    owner
        .execute(Command::DoBatch(operations), origin(2), vec![], None)
        .unwrap();
    let initial = owner.history().charge();
    owner
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    owner
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            }),
            origin(4),
            vec![],
            None,
        )
        .unwrap();
    (owner, initial)
}
#[test]
fn ruling6_creation_information_is_not_in_current_records() {
    let (short, short_initial) = sizing_probe_discarded_delta(1);
    let (long, long_initial) = sizing_probe_discarded_delta(64);
    assert_ne!(
        short_initial, long_initial,
        "INITIAL_CANDIDATE_CHARGES_DIFFER"
    );
    let short_bytes = sizing_probe_record(&short);
    assert_eq!(
        short_bytes,
        sizing_probe_record(&long),
        "DISCARDED_INITIAL_DELTA_IS_NOT_RECOVERABLE_FROM_RECORD"
    );
    eprintln!(
        "RULING6_DISCARDED_DELTA initial_short={short_initial:?} initial_long={long_initial:?} identical_record_bytes={}",
        short_bytes.len()
    );

    let compact = sizing_probe_owner(Role::Task, 0);
    let spare = sizing_probe_owner(Role::Task, 8192);
    let compact_live =
        kinewright_core::undo::document_capacity_bytes(compact.history().baseline()).unwrap();
    let spare_live =
        kinewright_core::undo::document_capacity_bytes(spare.history().baseline()).unwrap();
    assert!(
        spare_live > compact_live,
        "BASELINE_LIVE_MEASUREMENTS_DIFFER"
    );
    let compact_bytes = sizing_probe_record(&compact);
    assert_eq!(
        compact_bytes,
        sizing_probe_record(&spare),
        "BASELINE_SPARE_CAPACITY_IS_NOT_RECOVERABLE_FROM_RECORD"
    );
    eprintln!(
        "RULING6_BASELINE_CAPACITY compact_live={compact_live} spare_live={spare_live} identical_record_bytes={}",
        compact_bytes.len()
    );

    let mut limits = PoolLimits::default();
    let ordinary = Role::Ordinary.envelope().retained;
    limits.retained[0] = ordinary + (24 << 20);
    let pool = HistoryPool::new(limits);
    let owner = OwnerHistory::new(
        &pool,
        Role::Ordinary,
        Role::Ordinary.policy(),
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
    let before = pool.usage(Role::Ordinary).unwrap();
    assert!(
        matches!(owner.capture(), Err(Error::Reservation)),
        "ORDINARY_CAPTURE_NEEDS_A_SECOND_FULL_LEASE"
    );
    assert_eq!(
        pool.usage(Role::Ordinary).unwrap(),
        before,
        "REFUSED_CAPTURE_LEAVES_THE_SOURCE_RESERVATION_UNCHANGED"
    );
    assert!(104 + 104 > 128);
    eprintln!(
        "RULING6_ORDINARY_CAPTURE current_envelope={ordinary} spare_pool_room={} capture=Reservation proposed_envelope_mib=104 proposed_pool_mib=128 minimum_two_leases_mib=208",
        24 << 20
    );
}
