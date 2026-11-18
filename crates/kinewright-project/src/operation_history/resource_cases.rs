use kinewright_core::{AssetId, Document, Operation, ThreePointMode, TimeCode, TrackId};

#[derive(Clone, Copy, Debug)]
pub(super) enum ImplicitEdit {
    Insert,
    PatchedInsert,
    Overwrite,
}
impl ImplicitEdit {
    pub const ALL: [Self; 3] = [Self::Insert, Self::PatchedInsert, Self::Overwrite];
}
// Only the Linux store reproductions use this fixed case.
#[cfg(target_os = "linux")]
pub(super) fn implicit_title_insert() -> (Document, Operation) {
    implicit_title_case(ImplicitEdit::Insert)
}
pub(super) fn implicit_title_case(kind: ImplicitEdit) -> (Document, Operation) {
    let mut cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../kinewright-core/tests/support/history_operation_cases.json"
    ))
    .unwrap();
    let mut document: Document =
        serde_json::from_value(cases["ThreePointEdit"]["document"].take()).unwrap();
    cases["AddTitle"]["operation"]["AddTitle"]["duration"] = 120.into();
    let title = serde_json::from_value(cases["AddTitle"]["operation"].take()).unwrap();
    kinewright_core::apply_batch(&mut document, &[title]).unwrap();
    let operation = match kind {
        ImplicitEdit::Insert | ImplicitEdit::Overwrite => Operation::ThreePointEdit {
            track: TrackId(1),
            asset: AssetId(1),
            source_in: Some(TimeCode(10)),
            source_out: Some(TimeCode(40)),
            timeline_in: Some(TimeCode(50)),
            timeline_out: None,
            mode: if matches!(kind, ImplicitEdit::Insert) {
                ThreePointMode::Insert
            } else {
                ThreePointMode::Overwrite
            },
        },
        ImplicitEdit::PatchedInsert => Operation::PatchedThreePointEdit {
            asset: AssetId(1),
            source_in: Some(TimeCode(10)),
            source_out: Some(TimeCode(40)),
            timeline_in: Some(TimeCode(50)),
            timeline_out: None,
            mode: ThreePointMode::Insert,
            video_track: Some(TrackId(1)),
            audio_track: None,
        },
    };
    (document, operation)
}

pub(super) fn title_half(document: &Document, original: bool) -> kinewright_core::ClipId {
    document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .find(|clip| {
            matches!(clip.content, kinewright_core::ClipContent::Title(_))
                && (clip.id == kinewright_core::ClipId(1)) == original
        })
        .expect("IMPLICIT_EDIT_PRESERVES_BOTH_TITLE_HALVES")
        .id
}

pub(super) fn session_operations(kind: ImplicitEdit) -> (Vec<Operation>, Operation) {
    let (mut document, mut insert) = implicit_title_case(kind);
    let asset = document.media_pool.remove(0);
    let kinewright_core::ClipContent::Title(title) = document.tracks[0].clips.remove(0).content
    else {
        unreachable!("the implicit title fixture starts with its title clip")
    };
    match &mut insert {
        Operation::ThreePointEdit { track, .. } => *track = TrackId(81),
        Operation::PatchedThreePointEdit { video_track, .. } => *video_track = Some(TrackId(81)),
        _ => unreachable!(),
    }
    (
        vec![
            Operation::AddAsset { asset },
            Operation::AddTitle {
                track: TrackId(81),
                at: TimeCode(0),
                duration: TimeCode(120),
                title,
            },
        ],
        insert,
    )
}
