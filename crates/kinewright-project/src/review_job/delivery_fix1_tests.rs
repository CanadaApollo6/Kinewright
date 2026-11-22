// JA-1 S6 C2 fix 1. Included from `review_job/tests.rs` after
// `delivery_tests.rs`, whose fixtures it uses. Linux only, like the module.

/// The hidden staging folder of the running save, by name.
fn s6f1_staging(out: &std::path::Path) -> String {
    let names: Vec<String> = std::fs::read_dir(out)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with('.') && name.contains(".partial-"))
        .collect();
    assert_eq!(names.len(), 1, "S6F1_ONE_STAGING_FOLDER: {names:?}");
    names[0].clone()
}

/// Move the staging folder aside and put an empty folder under its name.
fn s6f1_swap(out: &std::path::Path) -> String {
    let staging = s6f1_staging(out);
    std::fs::rename(out.join(&staging), out.join("moved-aside")).unwrap();
    std::fs::create_dir(out.join(&staging)).unwrap();
    staging
}

/// Neon F1: a symlink the person picked is followed, and the folder is
/// judged by where it really is.
#[test]
fn ja1_s6_c2_fix1_a_symlinked_folder_is_judged_by_its_real_place() {
    let fixture = s6_sealed("s6c2f1-links");
    let (_, store) = fixture.refused_roots();
    let nested = store.join("studio").join("objects");
    let inside = fixture.temp.path("link-in");
    std::os::unix::fs::symlink(&nested, &inside).unwrap();
    let outside = fixture.temp.path("link-out");
    std::os::unix::fs::symlink(&fixture.out, &outside).unwrap();
    for folder in [inside.clone(), inside.join("")] {
        let Err(error) = fixture.destination(&folder) else {
            panic!("S6F1_LINK_INTO_STORE_ADMITTED: {}", folder.display());
        };
        assert_eq!(s6_identity(&error), INSIDE_PROJECT, "S6F1_LINK_INTO_STORE");
    }
    for folder in [outside.clone(), outside.join("")] {
        let destination = fixture
            .destination(&folder)
            .unwrap_or_else(|error| panic!("S6F1_LINK_OUT_REFUSED {}: {error}", folder.display()));
        assert_eq!(
            destination.display(),
            fixture.out.canonicalize().unwrap(),
            "S6F1_SHOWN_AT_ITS_REAL_PLACE"
        );
    }
}

/// Neon F1: a refused root whose metadata cannot be read for any reason
/// other than its absence refuses the save.
#[test]
fn ja1_s6_c2_fix1_an_unreadable_refused_root_refuses() {
    let fixture = s6_sealed("s6c2f1-root");
    let looped = fixture.temp.path("loop");
    std::os::unix::fs::symlink(&looped, &looped).unwrap();
    let missing = fixture.temp.path("missing");
    let Err(error) = admit_save_folder(&fixture.out, &[&looped]) else {
        panic!("S6F1_UNREADABLE_ROOT_SKIPPED");
    };
    assert_eq!(
        s6_identity(&error),
        super::delivery::ROOT_UNCHECKED,
        "S6F1_UNREADABLE_ROOT"
    );
    // Only an absent root is skipped.
    assert!(admit_save_folder(&fixture.out, &[&missing]).is_ok());
}

/// Neon F2: a folder swapped into the staging name before cleanup is not
/// removed, and the save's own files go through its held handle.
#[test]
fn ja1_s6_c2_fix1_cleanup_leaves_a_folder_swapped_into_the_staging_name() {
    let fixture = s6_sealed("s6c2f1-swap-clean");
    let mut swapped = None;
    let error = fixture
        .save_with(&ExportCancellation::default(), &mut |_, _| {
            swapped = Some(s6f1_swap(&fixture.out));
            Err(std::io::ErrorKind::StorageFull.into())
        })
        .unwrap_err();
    assert!(matches!(error, BundleError::Io(_)), "S6F1_FAILED: {error:?}");
    let swapped = swapped.expect("S6F1_SWAPPED");
    assert!(
        fixture.out.join(&swapped).is_dir(),
        "S6F1_SWAPPED_FOLDER_UNTOUCHED"
    );
    // The save's own folder was emptied through its handle.
    let aside: Vec<_> = std::fs::read_dir(fixture.out.join("moved-aside"))
        .unwrap()
        .collect();
    assert!(aside.is_empty(), "S6F1_OWN_FILES_REMOVED: {aside:?}");
}

/// Neon F2: the rename publishes only the folder that was hashed and synced.
#[test]
fn ja1_s6_c2_fix1_a_swapped_staging_folder_is_never_published() {
    let fixture = s6_sealed("s6c2f1-swap-publish");
    let mut swapped = None;
    let error = fixture
        .save_with(&ExportCancellation::default(), &mut |_, _| {
            if swapped.is_none() {
                swapped = Some(s6f1_swap(&fixture.out));
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(
        s6_identity(&error),
        super::delivery::STAGING_CHANGED,
        "S6F1_SWAP_REFUSED"
    );
    let swapped = swapped.expect("S6F1_SWAPPED");
    assert!(
        !fixture.out.join(fixture.name()).exists(),
        "S6F1_NOTHING_PUBLISHED"
    );
    assert!(
        fixture.out.join(&swapped).is_dir(),
        "S6F1_SWAPPED_FOLDER_NOT_MOVED"
    );
}

/// GLM M1 / Neon F5: a receipt on an unsettled root may still roll back, so
/// it is never pinned for a save.
#[test]
fn ja1_s6_c2_fix1_an_unsettled_root_refuses_the_pin() {
    let fixture = s6_sealed("s6c2f1-unsettled");
    let S6Sealed {
        temp,
        mut bundle,
        receipt,
        ..
    } = fixture;
    let outcome = crate::io_observer::fail_once("root_parent_sync", || {
        bundle.save_as_with_limits(&temp.path("copy.kinewright"), StorageLimits::default())
    })
    .unwrap();
    let crate::bundle_copy::SaveAsOutcome::Copied(mut copy) = outcome else {
        panic!("S6F1_EXPECTED_A_COPY");
    };
    let target = copy.destination_owner_mut().unwrap();
    assert!(target.review_root_unsettled(), "S6F1_COPY_UNSETTLED");
    assert!(
        matches!(
            target.pin_sealed_cut(
                &receipt.key.snapshot,
                CoverageProfileId::OutputReviewV1,
                &receipt.key_sha256,
            ),
            Err(BundleError::Unsettled)
        ),
        "S6F1_UNSETTLED_REFUSED"
    );
    // The copy above also cannot read its version index, so the guard is
    // witnessed alone on a root whose reads still work: only the guard
    // stands between this receipt and a pin.
    let fixture = s6_sealed("s6c2f1-unsettled-flag");
    let S6Sealed {
        mut bundle,
        receipt,
        ..
    } = fixture;
    bundle.unsettled = true;
    let pins = |bundle: &ProjectBundle| bundle.store.retention_probe().unwrap().1.len();
    let before = pins(&bundle);
    let refused = bundle
        .pin_sealed_cut(
            &receipt.key.snapshot,
            CoverageProfileId::OutputReviewV1,
            &receipt.key_sha256,
        )
        .err();
    assert!(
        matches!(refused, Some(BundleError::Unsettled)),
        "S6F1_UNSETTLED_FLAG_REFUSED: {refused:?}"
    );
    assert_eq!(pins(&bundle), before, "S6F1_UNSETTLED_PINS_NOTHING");
    bundle.unsettled = false;
}

/// Neon F5: the free-space check refuses before any staging folder exists.
#[test]
fn ja1_s6_c2_fix1_short_space_is_refused_before_staging() {
    let fixture = s6_sealed("s6c2f1-space");
    let media = fixture.receipt.key.media_ref.byte_length;
    let short = fixture.destination(&fixture.out).unwrap().with_available(media);
    let mut copied = false;
    let error = fixture
        .pin()
        .save(
            &short,
            &fixture.name(),
            &ExportCancellation::default(),
            &mut |_, _| {
                copied = true;
                Ok(())
            },
        )
        .unwrap_err();
    assert!(
        matches!(error, BundleError::Capacity(_)),
        "S6F1_SHORT_SPACE: {error:?}"
    );
    assert!(!copied, "S6F1_NO_COPY_ON_SHORT_SPACE");
    assert!(fixture.entries().is_empty(), "S6F1_NO_STAGING_ON_SHORT_SPACE");
    let room = fixture
        .destination(&fixture.out)
        .unwrap()
        .with_available(media.saturating_mul(2).saturating_add(4 << 20));
    fixture
        .pin()
        .save(
            &room,
            &fixture.name(),
            &ExportCancellation::default(),
            &mut s6_no_op(),
        )
        .expect("S6F1_ROOM_SAVES");
}

/// Neon F5: the sealed objects stay pinned across a collection during the
/// copy (the saved round's hash proves the media survived it), and the pin
/// is released on success, failure and Stop, after which collection runs.
#[test]
fn ja1_s6_c2_fix1_the_pin_holds_across_a_collection_and_is_released() {
    let mut fixture = s6_sealed("s6c2f1-pin");
    let media = fixture.receipt.key.media_ref.sha256.clone();
    let count = |bundle: &ProjectBundle| -> u64 {
        bundle
            .store
            .retention_probe()
            .unwrap()
            .1
            .get(&media)
            .copied()
            .unwrap_or(0)
    };
    let before = count(&fixture.bundle);
    let name = fixture.name();
    for (round, ending) in ["saved", "failed", "stopped"].into_iter().enumerate() {
        let out = fixture.temp.path(&format!("pin-{round}"));
        std::fs::create_dir(&out).unwrap();
        let destination = fixture.destination(&out).unwrap();
        let cut = fixture.pin();
        let cancel = ExportCancellation::default();
        let bundle = &mut fixture.bundle;
        let mut during = None;
        let result = cut.save(&destination, &name, &cancel, &mut |_, chunk| {
            if during.is_none() {
                during = Some(count(bundle));
                // A collection while the pin is held may refuse; it never
                // takes the media the copy is reading.
                let _ = bundle.collect_after_ack();
            }
            match ending {
                "failed" => chunk[0] ^= 0xff,
                "stopped" => cancel.cancel(),
                _ => {}
            }
            Ok(())
        });
        assert_eq!(result.is_ok(), ending == "saved", "S6F1_{ending}: {result:?}");
        assert!(
            during.is_some_and(|during| during > before),
            "S6F1_PINNED_DURING_THE_COPY ({ending}): {during:?} vs {before}"
        );
        drop(cut);
        assert_eq!(count(&fixture.bundle), before, "S6F1_RELEASED_{ending}");
        fixture
            .bundle
            .collect_after_ack()
            .unwrap_or_else(|error| panic!("S6F1_COLLECTS_AFTER_{ending}: {error}"));
    }
}
