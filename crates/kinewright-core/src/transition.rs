/// The compositor behavior associated with a built-in transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionShading {
    /// Ramp the entering layer's alpha from transparent to fully visible.
    CrossfadeAlpha,
    /// Mix a solid black or white frame into the entering layer while keeping it opaque.
    FadeFromColor { white: bool },
    /// MO2 R4/R21: the entering layer slides in from its entry edge and the
    /// below-stack is pushed out the opposite edge.
    Push { axis: TransitionAxis, sign: i8 },
    /// MO2 R4/R21: the entering layer slides in over a stationary below-stack.
    Slide { axis: TransitionAxis, sign: i8 },
    /// MO2 R4/R21: a hard-edged reveal of the stationary entering layer.
    Wipe { axis: TransitionAxis, sign: i8 },
}

/// MO2 R4: the screen axis a geometric transition moves along. `sign` is the
/// entry edge: −1 enters from the left/top, +1 from the right/bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionAxis {
    Horizontal,
    Vertical,
}

/// The complete public contract for one built-in transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub shading: TransitionShading,
}

/// Built-in transition metadata used by validation, rendering, UI, and agent documentation.
///
/// MO2 R4: the M20 three plus 12 push/slide/wipe rows; direction names the
/// entry edge (`push_left` enters from the left).
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
    TransitionDescriptor {
        name: "push_left",
        description: "Enters from the left edge while pushing lower layers out the right edge.",
        shading: TransitionShading::Push {
            axis: TransitionAxis::Horizontal,
            sign: -1,
        },
    },
    TransitionDescriptor {
        name: "push_right",
        description: "Enters from the right edge while pushing lower layers out the left edge.",
        shading: TransitionShading::Push {
            axis: TransitionAxis::Horizontal,
            sign: 1,
        },
    },
    TransitionDescriptor {
        name: "push_up",
        description: "Enters from the top edge while pushing lower layers out the bottom edge.",
        shading: TransitionShading::Push {
            axis: TransitionAxis::Vertical,
            sign: -1,
        },
    },
    TransitionDescriptor {
        name: "push_down",
        description: "Enters from the bottom edge while pushing lower layers out the top edge.",
        shading: TransitionShading::Push {
            axis: TransitionAxis::Vertical,
            sign: 1,
        },
    },
    TransitionDescriptor {
        name: "slide_left",
        description: "Slides in from the left edge over stationary lower layers.",
        shading: TransitionShading::Slide {
            axis: TransitionAxis::Horizontal,
            sign: -1,
        },
    },
    TransitionDescriptor {
        name: "slide_right",
        description: "Slides in from the right edge over stationary lower layers.",
        shading: TransitionShading::Slide {
            axis: TransitionAxis::Horizontal,
            sign: 1,
        },
    },
    TransitionDescriptor {
        name: "slide_up",
        description: "Slides in from the top edge over stationary lower layers.",
        shading: TransitionShading::Slide {
            axis: TransitionAxis::Vertical,
            sign: -1,
        },
    },
    TransitionDescriptor {
        name: "slide_down",
        description: "Slides in from the bottom edge over stationary lower layers.",
        shading: TransitionShading::Slide {
            axis: TransitionAxis::Vertical,
            sign: 1,
        },
    },
    TransitionDescriptor {
        name: "wipe_left",
        description: "Reveals the stationary clip with a hard edge moving from the left.",
        shading: TransitionShading::Wipe {
            axis: TransitionAxis::Horizontal,
            sign: -1,
        },
    },
    TransitionDescriptor {
        name: "wipe_right",
        description: "Reveals the stationary clip with a hard edge moving from the right.",
        shading: TransitionShading::Wipe {
            axis: TransitionAxis::Horizontal,
            sign: 1,
        },
    },
    TransitionDescriptor {
        name: "wipe_up",
        description: "Reveals the stationary clip with a hard edge moving from the top.",
        shading: TransitionShading::Wipe {
            axis: TransitionAxis::Vertical,
            sign: -1,
        },
    },
    TransitionDescriptor {
        name: "wipe_down",
        description: "Reveals the stationary clip with a hard edge moving from the bottom.",
        shading: TransitionShading::Wipe {
            axis: TransitionAxis::Vertical,
            sign: 1,
        },
    },
];

#[must_use]
pub fn transition_descriptor(name: &str) -> Option<TransitionDescriptor> {
    TRANSITION_DESCRIPTORS
        .iter()
        .copied()
        .find(|descriptor| descriptor.name == name)
}
