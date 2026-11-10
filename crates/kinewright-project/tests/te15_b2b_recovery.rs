//! Authenticated two-cut graphs exercise the production fresh-open reader.
#![cfg(all(target_os = "linux", feature = "test-util"))]
use kinewright_project::bundle::LoadedBundle;
use kinewright_project::bundle_recovery::{RecoveryAssessment, assess};
use std::io::Read;

fn unpack(name: &str) -> kinewright_media::test_support::TempDirectory {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "b2b-open",
    );
    let input = std::fs::File::open(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/te15-b2b")
            .join(format!("{name}.json.gz")),
    )
    .unwrap();
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(input)
        .take(2 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .unwrap();
    let archive: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let objects = temp.path("project.kinewright-assets/studio/objects");
    std::fs::create_dir_all(&objects).unwrap();
    for (hash, value) in archive["objects"].as_object().unwrap() {
        assert_eq!(hash.len(), 64);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
        let value = value.as_str().unwrap();
        assert_eq!(kinewright_hash::sha256_bytes(value.as_bytes()), *hash);
        std::fs::write(objects.join(hash), value).unwrap();
    }
    std::fs::write(
        temp.path("project.kinewright"),
        archive["root"].as_str().unwrap(),
    )
    .unwrap();
    temp
}
#[test]
fn te15_b2b_open_valid_consent_and_audits() {
    let temp = unpack("good");
    let path = temp.path("project.kinewright");
    assert!(LoadedBundle::read(&path).is_ok(), "B2B_VALID_STORED_CUTS");
    assert!(matches!(
        assess(&path).unwrap(),
        RecoveryAssessment::Verified(_)
    ));
}
fn refuses(name: &str) {
    let temp = unpack(name);
    let path = temp.path("project.kinewright");
    let before = std::fs::read(&path).unwrap();
    let error = LoadedBundle::read(&path)
        .err()
        .expect("B2B_OPEN_REFUSES_CHANGED_CONSENT_OR_AUDIT");
    assert!(
        error
            .to_string()
            .contains("stored run consent or append-only audit changed"),
        "B2B_CROSS_CUT_REFUSAL {error}"
    );
    match assess(&path).unwrap() {
        RecoveryAssessment::Quarantined { reason, .. } => assert!(
            reason.contains("stored run consent or append-only audit changed"),
            "B2B_RECOVERY_QUARANTINES_CROSS_CUT"
        ),
        RecoveryAssessment::Verified(_) => panic!("B2B_RECOVERY_REFUSES_CHANGED_CONSENT_OR_AUDIT"),
    }
    assert_eq!(std::fs::read(path).unwrap(), before, "B2B_OPEN_NO_REWRITE");
}
#[test]
fn te15_b2b_open_fixed_consent() {
    refuses("grant");
}
#[test]
fn te15_b2b_open_append_only_approvals() {
    refuses("approvals");
}
#[test]
fn te15_b2b_open_append_only_applications() {
    refuses("applications");
}
