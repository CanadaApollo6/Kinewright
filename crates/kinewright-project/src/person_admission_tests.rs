use crate::{
    bundle::BundleError, bundle_records::ProjectId, namespace::ProjectNamespace,
    storage_budget::StorageLimits, store::ProjectStore,
};
use kinewright_core::{Core, Document, Marker, MarkerId, TimeCode};
use kinewright_media::test_support::TempDirectory;

#[test]
fn run_limit_person_record_uses_project_capacity_without_releasing_the_run() {
    let temp = TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "person-record-admission",
    );
    let mut store = ProjectStore::new(
        ProjectNamespace::acquire(&temp.path("project.kinewright")).unwrap(),
        Some(ProjectId::fresh().unwrap()),
        None,
        StorageLimits::default(),
    )
    .unwrap();
    let document = Document {
        markers: (1..=60_000)
            .map(|id| Marker {
                id: MarkerId(id),
                position: TimeCode(0),
                label: String::new(),
                color_token: 0,
            })
            .collect(),
        ..Default::default()
    };
    let bytes = serde_json::to_vec(&document).unwrap();
    let decoded = crate::store::preflight_record(&bytes, store.limits()).unwrap();
    assert!(decoded > crate::run_admission::DECODED_BYTES);
    assert!(decoded < store.limits().decoded_record_bytes);
    store.set_run_budget(true);
    assert!(matches!(
        store.admit_document(&document),
        Err(BundleError::RunLimit)
    ));
    assert!(store.run_limit_reached());
    let mut person = store.person_publication_store();
    let sealed = person.admit_document(&document).unwrap();
    assert_eq!(
        *person.read_record::<Document>(&sealed.reference).unwrap(),
        document
    );
    let core = Core::spawn(Document {
        markers: (1..=2_000)
            .map(|id| Marker {
                id: MarkerId(id),
                position: TimeCode(0),
                label: String::new(),
                color_token: 0,
            })
            .collect(),
        ..Default::default()
    })
    .unwrap();
    let history = core.export_history().unwrap();
    let bytes = serde_json::to_vec(&history).unwrap();
    let decoded = crate::store::preflight_record(&bytes, store.limits()).unwrap();
    assert!(decoded > crate::run_admission::HISTORY_BYTES);
    assert!(decoded < store.limits().branch_history_bytes);
    let sealed = person.admit_history(&history).unwrap();
    assert_eq!(*person.resolve_history(&sealed.reference).unwrap(), history);
    assert!(store.run_limit_reached());
    assert!(matches!(
        store.admit_document(&Document::default()),
        Err(BundleError::RunLimit)
    ));
}

#[test]
fn run_limit_person_raw_refusal_is_capacity_and_does_not_latch() {
    let temp = TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "person-raw-capacity",
    );
    let mut store = ProjectStore::new(
        ProjectNamespace::acquire(&temp.path("project.kinewright")).unwrap(),
        Some(ProjectId::fresh().unwrap()),
        None,
        StorageLimits {
            branch_history_bytes: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let core = Core::spawn(Document::default()).unwrap();
    store.set_run_budget(true);
    let mut person = store.person_publication_store();
    assert!(matches!(
        person.admit_history(&core.export_history().unwrap()),
        Err(BundleError::Capacity("branch history"))
    ));
    assert!(!store.run_limit_reached());
    store.admit_document(&Document::default()).unwrap();
}
