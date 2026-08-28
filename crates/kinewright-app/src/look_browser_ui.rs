//! The CC4 §7 look browser: built-ins first, then this project's imported LUT
//! assets, each with title, size, provenance, and an availability chip.
//!
//! Selecting a look on a clip that already carries a `creative_look` retargets
//! that node; **Add as new look** stacks another one at the first index that
//! satisfies the stage order. Both are ordinary undoable operation batches —
//! the browser owns no rendering state and never touches the store.

use std::collections::BTreeMap;

use eframe::egui;
use kinewright_core::{
    Clip, ClipId, ColorNodeKind, ColorStage, Document, EffectId, LutAsset, LutAssetId,
    LutAvailabilityKind, LutAvailabilityStatus, LutNodeParams, Operation,
};
use kinewright_media::BuiltinLook;

use crate::{
    app::KinewrightApp,
    inspector_ui::{
        builtin_look_operations, builtin_retarget_operations, insert_lut_node_operation,
        lut_asset_param_operation, registered_builtin,
    },
    theme::{self, color, space, type_size},
};

/// Which look the browser row describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LookEntry {
    /// One of the five built-in generated assets (CC4 §2.6). Registered in
    /// `Document.lut_assets` on first use, never written to the store.
    Builtin(BuiltinLook),
    /// One asset the project already records.
    Project(LutAssetId),
}

/// One rendered browser row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LookRow {
    pub(crate) entry: LookEntry,
    pub(crate) title: String,
    pub(crate) size: u32,
    pub(crate) provenance: String,
    /// `None` for a built-in the project has not registered yet: it has no
    /// record to observe, and it becomes `verified` the moment it is added.
    pub(crate) availability: Option<LutAvailabilityKind>,
    /// Whether the currently targeted node already resolves to this look.
    pub(crate) selected: bool,
}

/// The browser's window state.
#[derive(Debug, Default)]
pub(crate) struct LookBrowserState {
    pub(crate) open: bool,
    /// The clip the browser acts on. `None` closes it: a look is always
    /// applied to a specific clip.
    pub(crate) clip: Option<ClipId>,
    /// The node a selection retargets. `None` means "retarget the clip's first
    /// `creative_look`, or insert one when it has none".
    pub(crate) effect: Option<EffectId>,
}

impl LookBrowserState {
    pub(crate) fn open_for(&mut self, clip: ClipId, effect: Option<EffectId>) {
        self.open = true;
        self.clip = Some(clip);
        self.effect = effect;
    }

    fn close(&mut self) {
        self.open = false;
        self.clip = None;
        self.effect = None;
    }
}

/// Every row the browser shows for one clip, built-ins first (CC4 §7).
///
/// A built-in the project already registered appears once, as a project asset
/// carrying its built-in provenance, rather than twice.
#[must_use]
pub(crate) fn look_rows(
    document: &Document,
    availability: &BTreeMap<LutAssetId, LutAvailabilityStatus>,
    bound: Option<LutAssetId>,
) -> Vec<LookRow> {
    let mut rows = Vec::with_capacity(BuiltinLook::ALL.len() + document.lut_assets.len());
    for builtin in BuiltinLook::ALL {
        if registered_builtin(document, builtin).is_some() {
            continue;
        }
        rows.push(LookRow {
            entry: LookEntry::Builtin(builtin),
            title: builtin.title().to_owned(),
            size: builtin.size(),
            provenance: format!("built-in · {}", builtin.name()),
            availability: None,
            selected: false,
        });
    }
    for asset in &document.lut_assets {
        rows.push(LookRow {
            entry: LookEntry::Project(asset.id),
            title: asset.title.clone(),
            size: asset.size,
            provenance: project_provenance(asset),
            availability: availability.get(&asset.id).map(|status| status.kind),
            selected: bound == Some(asset.id),
        });
    }
    rows
}

/// One project asset's provenance token: the built-in name, or the file stem
/// of the path the operator imported it from (informational only).
fn project_provenance(asset: &LutAsset) -> String {
    match &asset.source {
        kinewright_core::LutAssetSource::Builtin { name } => format!("built-in · {name}"),
        kinewright_core::LutAssetSource::Imported { source_path } => {
            std::path::Path::new(source_path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map_or_else(|| source_path.clone(), std::borrow::ToOwned::to_owned)
        }
    }
}

/// The clip's first `creative_look`, which a browser selection retargets when
/// the caller named no specific node.
#[must_use]
pub(crate) fn first_creative_look(clip: &Clip) -> Option<EffectId> {
    clip.effects
        .iter()
        .find(|effect| {
            ColorNodeKind::from_effect_name(&effect.name) == Some(ColorNodeKind::CreativeLook)
        })
        .map(|effect| effect.id)
}

/// The asset one node is bound to, for the browser's selection highlight.
#[must_use]
pub(crate) fn bound_asset(clip: &Clip, effect: Option<EffectId>) -> Option<LutAssetId> {
    let target = effect.or_else(|| first_creative_look(clip))?;
    let effect = clip
        .effects
        .iter()
        .find(|candidate| candidate.id == target)?;
    let params = LutNodeParams::from_effect(effect);
    (!params.is_unbound()).then_some(params.lut_asset_id)
}

/// Selecting a look retargets the existing node (CC4 §7).
///
/// A built-in the project does not carry yet is registered first, so the batch
/// is `[AddLutAsset, SetEffectParam]`; an asset already in the document is one
/// `SetEffectParam`. When the clip carries no `creative_look` at all, the
/// selection inserts one instead, because a look the operator picked must
/// become visible.
///
/// # Errors
///
/// Returns the human reason when the clip is gone or the id space is
/// exhausted.
pub(crate) fn select_look_operations(
    document: &Document,
    clip: &Clip,
    effect: Option<EffectId>,
    entry: &LookEntry,
) -> Result<Vec<Operation>, String> {
    let Some(target) = effect.or_else(|| first_creative_look(clip)) else {
        return add_look_operations(document, clip, entry);
    };
    match entry {
        LookEntry::Builtin(builtin) => {
            builtin_retarget_operations(document, clip.id, target, *builtin)
        }
        LookEntry::Project(asset) => {
            if document.lut_asset(*asset).is_none() {
                return Err(format!("LUT asset {asset} no longer exists"));
            }
            Ok(vec![lut_asset_param_operation(clip.id, target, *asset)])
        }
    }
}

/// **Add as new look** stacks another `creative_look` at the first legal index
/// (CC4 §7).
///
/// # Errors
///
/// Returns the human reason when the id space is exhausted or the asset is
/// gone.
pub(crate) fn add_look_operations(
    document: &Document,
    clip: &Clip,
    entry: &LookEntry,
) -> Result<Vec<Operation>, String> {
    match entry {
        LookEntry::Builtin(builtin) => {
            builtin_look_operations(document, clip, *builtin, ColorStage::Look)
        }
        LookEntry::Project(asset) => {
            if document.lut_asset(*asset).is_none() {
                return Err(format!("LUT asset {asset} no longer exists"));
            }
            Ok(vec![insert_lut_node_operation(
                clip,
                ColorStage::Look,
                *asset,
            )])
        }
    }
}

impl KinewrightApp {
    /// Draw the look browser window when it is open.
    pub(crate) fn look_browser(&mut self, ctx: &egui::Context) {
        if !self.look_browser.open {
            return;
        }
        let Some(clip_id) = self.look_browser.clip else {
            self.look_browser.close();
            return;
        };
        let document = std::sync::Arc::clone(&self.focused().document);
        let Some(clip) = document.clip(clip_id).cloned() else {
            self.look_browser.close();
            return;
        };
        let effect = self.look_browser.effect;
        let rows = look_rows(
            &document,
            &self.focused().lut_availability,
            bound_asset(&clip, effect),
        );
        let store_unavailable = self.focused().lut_store_unavailable_reason();
        let mut open = true;
        let mut pending: Option<Result<Vec<Operation>, String>> = None;
        let mut import = false;
        egui::Window::new("Looks")
            .open(&mut open)
            .resizable(true)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.colored_label(
                    color::TEXT_MUTED,
                    "Built-in looks first, then this project's imported LUT assets.",
                );
                ui.add_space(space::ONE);
                if ui
                    .add_enabled(
                        store_unavailable.is_none(),
                        egui::Button::new("Import LUT…"),
                    )
                    .on_hover_text(
                        store_unavailable.clone().unwrap_or_else(|| {
                            "Import a .cube into this project's store".to_owned()
                        }),
                    )
                    .clicked()
                {
                    import = true;
                }
                ui.add_space(space::ONE);
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for row in &rows {
                            if let Some(request) = look_row(ui, row) {
                                pending = Some(match request {
                                    RowAction::Select => {
                                        select_look_operations(&document, &clip, effect, &row.entry)
                                    }
                                    RowAction::AddAsNew => {
                                        add_look_operations(&document, &clip, &row.entry)
                                    }
                                });
                            }
                        }
                    });
            });
        if !open {
            self.look_browser.close();
        }
        if import {
            if let Some(path) = crate::media_workflow::choose_lut_file() {
                self.start_lut_import(
                    path,
                    crate::media_workflow::LutImportIntent::Apply {
                        clip: Some(clip_id),
                        stage: ColorStage::Look,
                    },
                );
            }
            return;
        }
        match pending {
            Some(Ok(operations)) => {
                self.send_operations(operations);
                self.look_browser.close();
            }
            Some(Err(reason)) => self.record_error("Look", reason),
            None => {}
        }
    }
}

/// Which button one browser row's operator pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowAction {
    /// Retarget the targeted node onto this look.
    Select,
    /// Stack another `creative_look` bound to this look.
    AddAsNew,
}

/// Draw one browser row and report the button that was pressed.
fn look_row(ui: &mut egui::Ui, row: &LookRow) -> Option<RowAction> {
    let mut action = None;
    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            let mut title = egui::RichText::new(&row.title).font(theme::semibold(type_size::BODY));
            if row.selected {
                title = title.color(color::ACCENT);
            }
            ui.label(title);
            ui.colored_label(
                color::TEXT_MUTED,
                format!("{}³ · {}", row.size, row.provenance),
            );
            let (chip_text, chip_color) = chip_for(row.availability);
            ui.colored_label(chip_color, chip_text);
        });
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Select").clicked() {
                action = Some(RowAction::Select);
            }
            if ui
                .small_button("Add as new look")
                .on_hover_text("Stack another creative look after the existing ones")
                .clicked()
            {
                action = Some(RowAction::AddAsNew);
            }
        });
    });
    action
}

/// One availability state as its browser chip, matching the media card's
/// warning treatment.
fn chip_for(kind: Option<LutAvailabilityKind>) -> (&'static str, egui::Color32) {
    match kind {
        Some(LutAvailabilityKind::Verified) => ("verified", color::STATUS_SUCCESS),
        Some(LutAvailabilityKind::Missing) => ("missing", color::STATUS_DANGER),
        Some(LutAvailabilityKind::Changed) => ("changed", color::STATUS_DANGER),
        Some(LutAvailabilityKind::Unreadable) => ("unreadable", color::STATUS_DANGER),
        None => ("not yet registered", color::TEXT_MUTED),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use kinewright_core::{
        AssetId, ClipContent, Effect, LUT_ASSET_ID_PARAMETER, LutAssetSource, ParamValue, TimeCode,
        Track, TrackId, TrackKind, apply_batch,
    };

    use super::*;

    fn clip_with(effects: Vec<Effect>) -> Clip {
        Clip {
            id: ClipId(10),
            asset: AssetId(1),
            timeline_start: TimeCode::ZERO,
            source_range: TimeCode::ZERO..TimeCode(24),
            content: ClipContent::Media,
            effects,
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }
    }

    fn look_effect(id: u64, asset: u64) -> Effect {
        Effect {
            id: EffectId(id),
            name: "creative_look".to_owned(),
            parameters: BTreeMap::from([(
                LUT_ASSET_ID_PARAMETER.to_owned(),
                ParamValue::Integer(i64::try_from(asset).expect("fixture id")),
            )]),
            keyframes: BTreeMap::new(),
        }
    }

    fn document_with(clip: Clip, assets: Vec<LutAsset>) -> Document {
        let fps = kinewright_core::Rational::new(24, 1).expect("valid fps");
        let mut document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![clip],
            }],
            media_pool: vec![kinewright_core::MediaAsset {
                id: AssetId(1),
                path: std::path::PathBuf::from("shot.mov"),
                name: "Shot".to_owned(),
                duration: TimeCode(24),
                fps,
                kind: kinewright_core::MediaKind::Video,
                resolution: Some((1920, 1080)),
                source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                color_description: kinewright_core::ColorDescription::default(),
            }],
            fps,
            resolution: (1920, 1080),
            lut_assets: assets,
            duration: TimeCode(24),
            ..Document::default()
        };
        document.color_context = kinewright_core::ColorContext::default();
        document
    }

    #[test]
    fn built_ins_come_first_and_a_registered_built_in_appears_only_once() {
        let document = document_with(
            clip_with(Vec::new()),
            vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(1))],
        );
        let rows = look_rows(&document, &BTreeMap::new(), None);

        // Four unregistered built-ins, then every project asset.
        assert_eq!(rows.len(), BuiltinLook::ALL.len() - 1 + 1);
        let builtin_rows = rows
            .iter()
            .take_while(|row| matches!(row.entry, LookEntry::Builtin(_)))
            .count();
        assert_eq!(builtin_rows, BuiltinLook::ALL.len() - 1);
        assert!(
            !rows
                .iter()
                .any(|row| row.entry == LookEntry::Builtin(BuiltinLook::Warm)),
            "a registered built-in must not be offered twice"
        );
        let project = rows.last().expect("the project asset row");
        assert_eq!(project.entry, LookEntry::Project(LutAssetId(1)));
        assert_eq!(project.title, BuiltinLook::Warm.title());
        assert!(project.provenance.contains("built-in"));
    }

    #[test]
    fn the_row_bound_to_the_targeted_node_is_marked_selected() {
        let clip = clip_with(vec![look_effect(2, 1)]);
        let document = document_with(
            clip.clone(),
            vec![BuiltinLook::Cool.to_lut_asset(LutAssetId(1))],
        );
        assert_eq!(bound_asset(&clip, None), Some(LutAssetId(1)));
        assert_eq!(first_creative_look(&clip), Some(EffectId(2)));
        let rows = look_rows(&document, &BTreeMap::new(), bound_asset(&clip, None));
        assert_eq!(rows.iter().filter(|row| row.selected).count(), 1);
        assert!(
            rows.iter()
                .find(|row| row.selected)
                .is_some_and(|row| row.entry == LookEntry::Project(LutAssetId(1)))
        );
    }

    #[test]
    fn selecting_a_project_asset_retargets_the_existing_node() {
        let clip = clip_with(vec![look_effect(2, 1)]);
        let document = document_with(
            clip.clone(),
            vec![
                BuiltinLook::Warm.to_lut_asset(LutAssetId(1)),
                BuiltinLook::Cool.to_lut_asset(LutAssetId(2)),
            ],
        );
        let operations =
            select_look_operations(&document, &clip, None, &LookEntry::Project(LutAssetId(2)))
                .expect("retarget builds");
        assert_eq!(
            operations,
            vec![Operation::SetEffectParam {
                clip: ClipId(10),
                effect: EffectId(2),
                name: LUT_ASSET_ID_PARAMETER.to_owned(),
                value: ParamValue::Integer(2),
            }]
        );
    }

    #[test]
    fn selecting_an_unregistered_built_in_registers_it_then_retargets() {
        let clip = clip_with(vec![look_effect(2, 1)]);
        let mut document = document_with(
            clip.clone(),
            vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(1))],
        );
        let operations = select_look_operations(
            &document,
            &clip,
            None,
            &LookEntry::Builtin(BuiltinLook::Monochrome),
        )
        .expect("retarget builds");
        assert_eq!(operations.len(), 2);
        let Operation::AddLutAsset { asset } = &operations[0] else {
            panic!("the batch must lead with AddLutAsset");
        };
        assert_eq!(asset.id, LutAssetId(2));
        assert_eq!(
            asset.source,
            LutAssetSource::Builtin {
                name: "monochrome".to_owned()
            }
        );
        for operation in &operations {
            apply_batch(&mut document, std::slice::from_ref(operation))
                .unwrap_or_else(|error| panic!("{operation:?} rejected: {error}"));
        }
        let bound = bound_asset(document.clip(ClipId(10)).expect("clip"), None);
        assert_eq!(bound, Some(LutAssetId(2)));
        document
            .validate()
            .expect("the retargeted document is valid");
    }

    #[test]
    fn add_as_new_look_stacks_a_second_node_after_the_first() {
        let clip = clip_with(vec![look_effect(2, 1)]);
        let mut document = document_with(
            clip.clone(),
            vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(1))],
        );
        let operations = add_look_operations(&document, &clip, &LookEntry::Project(LutAssetId(1)))
            .expect("add builds");
        assert_eq!(
            operations,
            vec![Operation::InsertEffect {
                clip: ClipId(10),
                index: 1,
                effect: Effect {
                    id: EffectId(3),
                    name: "creative_look".to_owned(),
                    parameters: BTreeMap::from([(
                        LUT_ASSET_ID_PARAMETER.to_owned(),
                        ParamValue::Integer(1)
                    )]),
                    keyframes: BTreeMap::new(),
                },
            }]
        );
        for operation in &operations {
            apply_batch(&mut document, std::slice::from_ref(operation))
                .expect("Core accepts the stacked look");
        }
        assert_eq!(document.clip(ClipId(10)).expect("clip").effects.len(), 2);
        document.validate().expect("two looks are a valid stack");
    }

    #[test]
    fn selecting_a_look_on_a_clip_with_no_node_inserts_one() {
        let clip = clip_with(Vec::new());
        let document = document_with(
            clip.clone(),
            vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(1))],
        );
        let operations =
            select_look_operations(&document, &clip, None, &LookEntry::Project(LutAssetId(1)))
                .expect("select builds");
        assert!(matches!(
            operations.as_slice(),
            [Operation::InsertEffect { index: 0, .. }]
        ));
    }

    #[test]
    fn a_vanished_asset_is_refused_rather_than_bound() {
        let clip = clip_with(vec![look_effect(2, 1)]);
        let document = document_with(clip.clone(), Vec::new());
        assert!(
            select_look_operations(&document, &clip, None, &LookEntry::Project(LutAssetId(9)))
                .expect_err("a dangling id is refused")
                .contains('9')
        );
        assert!(
            add_look_operations(&document, &clip, &LookEntry::Project(LutAssetId(9)))
                .expect_err("a dangling id is refused")
                .contains('9')
        );
    }

    // -----------------------------------------------------------------------
    // CC7 §6 (e) — the person path for the creative look
    // -----------------------------------------------------------------------

    use kinewright_core::cc7_scenarios::{
        CC7_E_OPERATIONS, CC7_LOOK_MIX_BASIS_POINTS, CC7_LUT_ASSET_ID, CC7_SOURCE_FPS,
        CC7_SOURCE_FRAMES, CC7_SOURCE_HEIGHT, CC7_SOURCE_WIDTH, Cc7Scenario,
        cc7_canonical_operations, cc7_lut_backed_canonical_operations, cc7_spec, cc7_target_clip,
    };

    /// CC7 §2.3.5's one-clip (e) document, carrying no effect and no asset.
    fn cc7_look_document() -> (Document, Clip) {
        let spec = cc7_spec(Cc7Scenario::CreativeLook);
        let fps = kinewright_core::Rational::new(CC7_SOURCE_FPS, 1).expect("the CC7 rate is valid");
        let length = TimeCode(i64::from(CC7_SOURCE_FRAMES));
        let clip = Clip {
            id: cc7_target_clip(Cc7Scenario::CreativeLook),
            asset: AssetId(1),
            source_range: TimeCode::ZERO..length,
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        };
        let mut document = Document {
            media_pool: vec![kinewright_core::MediaAsset {
                id: AssetId(1),
                path: std::path::PathBuf::from("cc7-e-0.mkv"),
                name: spec.title.to_owned(),
                duration: length,
                fps,
                kind: kinewright_core::MediaKind::Video,
                resolution: Some((CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT)),
                source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                color_description: kinewright_core::ColorDescription::default(),
            }],
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![clip.clone()],
            }],
            fps,
            resolution: (CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT),
            lut_assets: Vec::new(),
            duration: length,
            ..Document::default()
        };
        document.color_context = kinewright_core::ColorContext::default();
        (document, clip)
    }

    /// CC7 §6 (e): **Add as new look** on the built-in `warm` asset registers
    /// it and stacks one `creative_look` carrying **only** `lut_asset_id`.
    ///
    /// `mix_basis_points = 10 000` is the neutral (CC4 §5), so the card's mix
    /// row shows full strength without storing it — driven headlessly here, so
    /// the claim is about the control the person sees rather than about the
    /// document alone.
    #[test]
    fn cc7_e_a_person_can_add_the_built_in_warm_look() {
        let scenario = Cc7Scenario::CreativeLook;
        let (mut document, clip) = cc7_look_document();
        let asset = BuiltinLook::Warm.to_lut_asset(CC7_LUT_ASSET_ID);
        let canonical = cc7_lut_backed_canonical_operations(scenario, asset.clone());

        // The browser row the person clicks, and the builder behind it.
        let rows = look_rows(&document, &BTreeMap::new(), None);
        let warm = rows
            .iter()
            .find(|row| row.entry == LookEntry::Builtin(BuiltinLook::Warm))
            .expect("the warm built-in is offered on an empty project");
        assert!(!warm.selected && warm.availability.is_none());

        let operations =
            add_look_operations(&document, &clip, &warm.entry).expect("the add batch builds");
        assert_eq!(
            operations,
            builtin_look_operations(&document, &clip, BuiltinLook::Warm, ColorStage::Look)
                .expect("the §6 builder"),
            "**Add as new look** is `builtin_look_operations` at the look stage"
        );
        assert_eq!(
            operations, canonical,
            "the person's batch is the canonical (e) batch"
        );
        let Operation::AddLutAsset { asset: registered } = &operations[0] else {
            panic!("the batch must lead with AddLutAsset");
        };
        assert_eq!(registered, &asset);

        for operation in &operations {
            apply_batch(&mut document, std::slice::from_ref(operation))
                .unwrap_or_else(|error| panic!("core rejected {operation:?}: {error}"));
        }
        document.validate().expect("the look document is valid");

        let expected = {
            let (mut expected, _) = cc7_look_document();
            for operation in &canonical {
                apply_batch(&mut expected, std::slice::from_ref(operation)).expect("core accepts");
            }
            expected
        };
        expected
            .validate()
            .expect("the canonical document is valid");
        assert_eq!(document, expected);

        // The node stores the binding and nothing else, and the one node CC7
        // pins for (e) is the `creative_look` at the look stage.
        let node = document
            .clip(clip.id)
            .expect("the clip")
            .effects
            .first()
            .expect("the look the browser stacked")
            .clone();
        assert_eq!(node.name, ColorNodeKind::CreativeLook.effect_name());
        assert_eq!(
            node.parameters.keys().collect::<Vec<_>>(),
            vec![LUT_ASSET_ID_PARAMETER]
        );
        assert_eq!(
            CC7_E_OPERATIONS[0]
                .parameters
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>(),
            vec![LUT_ASSET_ID_PARAMETER]
        );
        assert_eq!(canonical, {
            let mut batch = vec![Operation::AddLutAsset {
                asset: asset.clone(),
            }];
            batch.extend(cc7_canonical_operations(scenario));
            batch
        });

        // The mix row: the neutral is shown, and untouched it writes nothing.
        let params = LutNodeParams::from_effect(&node);
        assert_eq!(params.mix_basis_points, CC7_LOOK_MIX_BASIS_POINTS);
        assert!(
            !node.parameters.contains_key("mix_basis_points"),
            "the neutral mix is resolved, never stored"
        );
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut pending = crate::inspector_ui::InspectorEdits::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            crate::inspector_ui::look_mix_row(ui, &clip, &node, params, &mut pending);
        });
        assert!(
            pending.operations().is_empty(),
            "drawing the mix row writes nothing to the document"
        );
        let painted = theme::painted_text(&output);
        assert!(
            painted
                .iter()
                .any(|line| line == &format!("{CC7_LOOK_MIX_BASIS_POINTS} bp")),
            "the row shows the resolved neutral beside the slider:\n{painted:#?}"
        );
    }
}
