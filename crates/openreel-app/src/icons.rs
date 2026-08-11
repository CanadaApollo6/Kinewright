use eframe::egui;

use crate::theme::{radius, size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Add,
    Alert,
    Chat,
    Delete,
    Export,
    Filmstrip,
    Folder,
    Import,
    Lock,
    Pause,
    Play,
    Redo,
    Send,
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
            Self::Add => "bytes://openreel/icons/add.svg",
            Self::Alert => "bytes://openreel/icons/alert.svg",
            Self::Chat => "bytes://openreel/icons/chat.svg",
            Self::Delete => "bytes://openreel/icons/delete.svg",
            Self::Export => "bytes://openreel/icons/export.svg",
            Self::Filmstrip => "bytes://openreel/icons/filmstrip.svg",
            Self::Folder => "bytes://openreel/icons/folder.svg",
            Self::Import => "bytes://openreel/icons/import.svg",
            Self::Lock => "bytes://openreel/icons/lock.svg",
            Self::Pause => "bytes://openreel/icons/pause.svg",
            Self::Play => "bytes://openreel/icons/play.svg",
            Self::Redo => "bytes://openreel/icons/redo.svg",
            Self::Send => "bytes://openreel/icons/send.svg",
            Self::Split => "bytes://openreel/icons/split.svg",
            Self::StepBack => "bytes://openreel/icons/step-back.svg",
            Self::StepForward => "bytes://openreel/icons/step-forward.svg",
            Self::Stop => "bytes://openreel/icons/stop.svg",
            Self::Undo => "bytes://openreel/icons/undo.svg",
            Self::Unlock => "bytes://openreel/icons/unlock.svg",
            Self::Waveform => "bytes://openreel/icons/waveform.svg",
        }
    }

    const fn bytes(self) -> &'static [u8] {
        match self {
            Self::Add => include_bytes!("../assets/icons/add.svg"),
            Self::Alert => include_bytes!("../assets/icons/alert.svg"),
            Self::Chat => include_bytes!("../assets/icons/chat.svg"),
            Self::Delete => include_bytes!("../assets/icons/delete.svg"),
            Self::Export => include_bytes!("../assets/icons/export.svg"),
            Self::Filmstrip => include_bytes!("../assets/icons/filmstrip.svg"),
            Self::Folder => include_bytes!("../assets/icons/folder.svg"),
            Self::Import => include_bytes!("../assets/icons/import.svg"),
            Self::Lock => include_bytes!("../assets/icons/lock.svg"),
            Self::Pause => include_bytes!("../assets/icons/pause.svg"),
            Self::Play => include_bytes!("../assets/icons/play.svg"),
            Self::Redo => include_bytes!("../assets/icons/redo.svg"),
            Self::Send => include_bytes!("../assets/icons/send.svg"),
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
