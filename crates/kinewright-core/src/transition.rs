/// The compositor behavior associated with a built-in transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionShading {
    /// Ramp the entering layer's alpha from transparent to fully visible.
    CrossfadeAlpha,
    /// Mix a solid black or white frame into the entering layer while keeping it opaque.
    FadeFromColor { white: bool },
}

/// The complete public contract for one built-in transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub shading: TransitionShading,
}

/// Built-in transition metadata used by validation, rendering, UI, and agent documentation.
pub const TRANSITION_DESCRIPTORS: &[TransitionDescriptor] = &[
    TransitionDescriptor {
        name: "crossfade",
        description: "Reveals lower layers or black while the entering clip alpha ramps to full.",
        shading: TransitionShading::CrossfadeAlpha,
    },
    TransitionDescriptor {
        name: "fade_from_black",
        description: "Starts as opaque black and fades to the entering clip, occluding lower layers.",
        shading: TransitionShading::FadeFromColor { white: false },
    },
    TransitionDescriptor {
        name: "fade_from_white",
        description: "Starts as opaque white and fades to the entering clip, occluding lower layers.",
        shading: TransitionShading::FadeFromColor { white: true },
    },
];

#[must_use]
pub fn transition_descriptor(name: &str) -> Option<TransitionDescriptor> {
    TRANSITION_DESCRIPTORS
        .iter()
        .copied()
        .find(|descriptor| descriptor.name == name)
}
