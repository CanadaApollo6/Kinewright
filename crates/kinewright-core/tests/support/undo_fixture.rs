use kinewright_core::{
    AutomationCurve, ClipId, Document, Keyframe, KeyframeInterpolation, TimeCode,
};

pub fn heavy() -> Document {
    let mut cases: serde_json::Value =
        serde_json::from_str(include_str!("history_operation_cases.json")).unwrap();
    let mut document: Document =
        serde_json::from_value(cases["SetEffectKeyframes"]["document"].take()).unwrap();
    let mut template = document.tracks[0].clips.pop().unwrap();
    template.audio_gain_curve = None;
    template.effects[0].keyframes.insert(
        "exposure_milli_stops".into(),
        AutomationCurve {
            keyframes: (0..8)
                .map(|i| Keyframe {
                    at: TimeCode(i * 8),
                    value: i * 20,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                })
                .collect(),
        },
    );
    let effect = template.effects.pop().unwrap();
    document.tracks[0].clips = (0..400)
        .map(|index| {
            // Each clip/effect owns independent fixture data, just as a loaded project does.
            let mut clip = template.clone();
            clip.id = ClipId(index + 1);
            clip.timeline_start = TimeCode(i64::try_from(index).unwrap() * 60);
            let count = if index == 0 {
                16
            } else {
                u64::from(index <= 384)
            };
            clip.effects = (0..count)
                .map(|id| {
                    let mut copy = effect.clone();
                    copy.id = kinewright_core::EffectId(id + 1);
                    copy
                })
                .collect();
            clip
        })
        .collect();
    document.tracks[0].sync_lock = true;
    document.duration = TimeCode(24_000);
    document.validate().unwrap();
    assert_eq!(document.tracks[0].clips.len(), 400);
    assert_eq!(
        document.tracks[0]
            .clips
            .iter()
            .map(|c| c.effects.len())
            .sum::<usize>(),
        400
    );
    assert_eq!(
        document.tracks[0]
            .clips
            .iter()
            .flat_map(|c| &c.effects)
            .flat_map(|e| e.keyframes.values())
            .map(|c| c.keyframes.len())
            .sum::<usize>(),
        3200
    );
    document
}
