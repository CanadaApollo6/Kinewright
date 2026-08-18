use eframe::egui;

use crate::theme::{radius, size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Add,
    Alert,
    /// Anthropic's Claude spark, in its brand terracotta (nominative use:
    /// it marks the session's harness, exactly as T3 Code does).
    BrandClaude,
    /// Cursor's cube mark, used only to identify the Cursor harness.
    BrandCursor,
    /// `OpenAI`'s blossom mark, white per its on-dark brand usage.
    BrandOpenAi,
    Delete,
    Export,
    Filmstrip,
    Folder,
    Import,
    Lock,
    Pause,
    Play,
    Record,
    Redo,
    Send,
    Settings,
    Split,
    StepBack,
    StepForward,
    Stop,
    Undo,
    Unlock,
    Waveform,
}

impl Icon {
    pub(crate) const fn uri(self) -> &'static str {
        match self {
            Self::Add => "bytes://kinewright/icons/add.svg",
            Self::Alert => "bytes://kinewright/icons/alert.svg",
            Self::BrandClaude => "bytes://kinewright/icons/brand-claude.svg",
            Self::BrandCursor => "bytes://kinewright/icons/brand-cursor.svg",
            Self::BrandOpenAi => "bytes://kinewright/icons/brand-openai.svg",
            Self::Delete => "bytes://kinewright/icons/delete.svg",
            Self::Export => "bytes://kinewright/icons/export.svg",
            Self::Filmstrip => "bytes://kinewright/icons/filmstrip.svg",
            Self::Folder => "bytes://kinewright/icons/folder.svg",
            Self::Import => "bytes://kinewright/icons/import.svg",
            Self::Lock => "bytes://kinewright/icons/lock.svg",
            Self::Pause => "bytes://kinewright/icons/pause.svg",
            Self::Play => "bytes://kinewright/icons/play.svg",
            Self::Record => "bytes://kinewright/icons/record.svg",
            Self::Redo => "bytes://kinewright/icons/redo.svg",
            Self::Send => "bytes://kinewright/icons/send.svg",
            Self::Settings => "bytes://kinewright/icons/settings.svg",
            Self::Split => "bytes://kinewright/icons/split.svg",
            Self::StepBack => "bytes://kinewright/icons/step-back.svg",
            Self::StepForward => "bytes://kinewright/icons/step-forward.svg",
            Self::Stop => "bytes://kinewright/icons/stop.svg",
            Self::Undo => "bytes://kinewright/icons/undo.svg",
            Self::Unlock => "bytes://kinewright/icons/unlock.svg",
            Self::Waveform => "bytes://kinewright/icons/waveform.svg",
        }
    }

    const fn bytes(self) -> &'static [u8] {
        match self {
            Self::Add => include_bytes!("../assets/icons/add.svg"),
            Self::Alert => include_bytes!("../assets/icons/alert.svg"),
            Self::BrandClaude => include_bytes!("../assets/icons/brand-claude.svg"),
            Self::BrandCursor => include_bytes!("../assets/icons/brand-cursor.svg"),
            Self::BrandOpenAi => include_bytes!("../assets/icons/brand-openai.svg"),
            Self::Delete => include_bytes!("../assets/icons/delete.svg"),
            Self::Export => include_bytes!("../assets/icons/export.svg"),
            Self::Filmstrip => include_bytes!("../assets/icons/filmstrip.svg"),
            Self::Folder => include_bytes!("../assets/icons/folder.svg"),
            Self::Import => include_bytes!("../assets/icons/import.svg"),
            Self::Lock => include_bytes!("../assets/icons/lock.svg"),
            Self::Pause => include_bytes!("../assets/icons/pause.svg"),
            Self::Play => include_bytes!("../assets/icons/play.svg"),
            Self::Record => include_bytes!("../assets/icons/record.svg"),
            Self::Redo => include_bytes!("../assets/icons/redo.svg"),
            Self::Send => include_bytes!("../assets/icons/send.svg"),
            Self::Settings => include_bytes!("../assets/icons/settings.svg"),
            Self::Split => include_bytes!("../assets/icons/split.svg"),
            Self::StepBack => include_bytes!("../assets/icons/step-back.svg"),
            Self::StepForward => include_bytes!("../assets/icons/step-forward.svg"),
            Self::Stop => include_bytes!("../assets/icons/stop.svg"),
            Self::Undo => include_bytes!("../assets/icons/undo.svg"),
            Self::Unlock => include_bytes!("../assets/icons/unlock.svg"),
            Self::Waveform => include_bytes!("../assets/icons/waveform.svg"),
        }
    }

    pub(crate) fn image(self, points: f32) -> egui::Image<'static> {
        egui::Image::from_bytes(self.uri(), self.bytes())
            .fit_to_exact_size(egui::vec2(points, points))
    }
}

pub(crate) fn button(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    ui.add(
        egui::Button::image(icon.image(size::ICON_MD))
            .image_tint_follows_text_color(true)
            .min_size(egui::vec2(size::ICON_BUTTON, size::ICON_BUTTON))
            .corner_radius(radius::SM),
    )
    .on_hover_text(label)
}

pub(crate) fn transport_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    selected: bool,
) -> egui::Response {
    ui.add(
        egui::Button::image(icon.image(size::ICON_LG))
            .image_tint_follows_text_color(true)
            .selected(selected)
            .min_size(egui::vec2(size::TRANSPORT_BUTTON, size::TRANSPORT_BUTTON))
            .corner_radius(radius::SM),
    )
    .on_hover_text(label)
}
