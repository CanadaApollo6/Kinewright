/// Exact sparse coverage, explicit targets and other keepers are independent.
#[test]
fn s3b_d14_only_unavailable_optional_continuation_is_removed() {
    for extra in [None, Some(5)] {
        let mut readers = Model::new(20).with_budget(100 * F);
        paused_step(&mut readers, 20, 0, 0);
        post_paused(&mut readers, &[(0, 19, 0)], HashMap::new());
        let id = readers.slots[0].id;
        let refill = loop {
            if let Next::Decode {
                at,
                version,
                bytes,
                from,
                ..
            } = step_reader(&mut readers, id)
            {
                assert_eq!((at, from), (19, Some(4)));
                break (at, version, bytes);
            }
        };
        let mut targets = vec![(0, 8, 0), (1, 0, 0)];
        if let Some(at) = extra {
            targets.push((0, at, 0));
        }
        post_paused(&mut readers, &targets, HashMap::new());
        let original_ring = readers.ring_times(&0);
        let original_window = readers.windows.clone();
        // Only 6 and 10 onward actually kept; 4/5/7 were optional promises.
        let kept: BTreeSet<i64> = std::iter::once(6).chain(10..19).collect();
        readers.refilled_exact(id, (refill.0, refill.1), &kept);
        let required: BTreeSet<i64> = readers
            .required
            .iter()
            .filter_map(|(key, at)| (*key == 0).then_some(*at))
            .collect();
        let expected: BTreeSet<i64> = [Some(6), Some(8), extra].into_iter().flatten().collect();
        assert_eq!(
            required, expected,
            "sparse exact coverage; every explicit target preserved"
        );
        assert!(readers.required.contains(&(1, 0)), "other source untouched");
        assert_eq!(readers.windows, original_window, "held window untouched");
        assert_eq!(readers.ring_times(&0), original_ring, "no ring eviction");
        deliver_ok(&mut readers, id, 0, refill);
        for slot in readers.slots.clone() {
            walk(&mut readers, slot.id, slot.key);
        }
        assert!(
            readers
                .resolve(
                    &targets
                        .iter()
                        .map(|(key, at, _)| (*key, *at))
                        .collect::<Vec<_>>()
                )
                .is_some()
        );
        assert!(readers.reserved.is_empty());
        let ring: usize = readers
            .rings
            .values()
            .flat_map(BTreeMap::values)
            .map(Fr::bytes)
            .sum();
        assert_eq!(readers.live().0, ring, "exact charges after progress");
    }
}

#[test]
fn s3b_d14_a_second_keeper_preserves_the_optional_time() {
    for ring_keeper in [false, true] {
        let mut readers = Model::new(20).with_budget(100 * F);
        let id = refilled_4_to_19(&mut readers);
        let ReaderState::Decoding { version, .. } = readers.slots[0].state else {
            panic!("refill in flight");
        };
        post_paused(&mut readers, &[(0, 8, 0)], HashMap::new());
        if ring_keeper {
            // An already delivered keeper: its reservation becomes the ring's charge.
            let bytes = readers.reserved.remove(&(0, 5)).expect("promised time");
            readers.release(bytes - F);
            readers.rings.entry(0).or_default().insert(
                5,
                Fr {
                    value: frame(0, 5),
                    bytes: F,
                    pinned: false,
                },
            );
        } else {
            // Scheduler ownership by a second same-source decoded keeper.
            let mut keeper = readers.slots[0].clone();
            keeper.id = 1000;
            keeper.state = ReaderState::Idle {
                since: Duration::ZERO,
            };
            keeper.retained = BTreeSet::from([5]);
            keeper.plan = Plan::default();
            keeper.dropping.clear();
            keeper.dispatched.clear();
            readers.slots.push(keeper);
        }
        let ring = readers.ring_times(&0);
        readers.refilled_exact(id, (19, version), &(10..19).collect());
        assert!(
            readers.required.contains(&(0, 5)),
            "another live keeper protects the optional time; ring={ring_keeper}"
        );
        assert!(
            !readers.required.contains(&(0, 4)),
            "unavailable optional neighbour removed"
        );
        assert!(!readers.required.contains(&(0, 6)));
        assert!(readers.required.contains(&(0, 8)), "explicit target stays");
        assert_eq!(readers.ring_times(&0), ring, "ring contents preserved");
    }
}
