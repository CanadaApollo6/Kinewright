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
    /// Meta's loop mark in brand blue, byte-identical to the Simple
    /// Icons glyph (nominative use: it marks the session's harness).
    BrandMuse,
    /// `OpenCode`'s pixel `o`, cropped from its official wordmark SVG with
    /// the official favicon fills (white ring, `#5A5858` counter).
    BrandOpenCode,
    /// Alibaba Qwen's current mark, cropped from the official
    /// `qwen-logo.svg` that `chat.qwen.ai` loads from Alibaba's CDN, with
    /// its official `#082DFF` fill and the wordmark letters dropped.
    BrandQwen,
    /// Kimi's K-only dark mark from Moonshot's official Branding-Guide
    /// repo (`k-only-dark.svg`), viewBox squared, otherwise as shipped.
    BrandKimi,
    /// Kiro's app icon from kiro.dev, used as shipped.
    BrandKiro,
    /// Devin's mark from devin.ai, white per on-dark usage.
    BrandDevin,
    /// GitHub Copilot's mark from Simple Icons, white per on-dark usage.
    BrandCopilot,
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
    /// Every icon, for the asset-coverage test. A new variant belongs here
    /// too; `uri` and `bytes` below are exhaustive matches, so they cannot
    /// fall behind, and the test checks this list against the asset folder.
    #[cfg(test)]
    const ALL: &'static [Self] = &[
        Self::Add,
        Self::Alert,
        Self::BrandClaude,
        Self::BrandCursor,
        Self::BrandOpenAi,
        Self::BrandMuse,
        Self::BrandOpenCode,
        Self::BrandQwen,
        Self::BrandKimi,
        Self::BrandKiro,
        Self::BrandDevin,
        Self::BrandCopilot,
        Self::Delete,
        Self::Export,
        Self::Filmstrip,
        Self::Folder,
        Self::Import,
        Self::Lock,
        Self::Pause,
        Self::Play,
        Self::Record,
        Self::Redo,
        Self::Send,
        Self::Settings,
        Self::Split,
        Self::StepBack,
        Self::StepForward,
        Self::Stop,
        Self::Undo,
        Self::Unlock,
        Self::Waveform,
    ];
}

impl Icon {
    pub(crate) const fn uri(self) -> &'static str {
        match self {
            Self::Add => "bytes://kinewright/icons/add.svg",
            Self::Alert => "bytes://kinewright/icons/alert.svg",
            Self::BrandClaude => "bytes://kinewright/icons/brand-claude.svg",
            Self::BrandCursor => "bytes://kinewright/icons/brand-cursor.svg",
            Self::BrandOpenAi => "bytes://kinewright/icons/brand-openai.svg",
            Self::BrandMuse => "bytes://kinewright/icons/brand-muse.svg",
            Self::BrandOpenCode => "bytes://kinewright/icons/brand-opencode.svg",
            Self::BrandQwen => "bytes://kinewright/icons/brand-qwen.svg",
            Self::BrandKimi => "bytes://kinewright/icons/brand-kimi.svg",
            Self::BrandKiro => "bytes://kinewright/icons/brand-kiro.svg",
            Self::BrandDevin => "bytes://kinewright/icons/brand-devin.svg",
            Self::BrandCopilot => "bytes://kinewright/icons/brand-copilot.svg",
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
            Self::BrandMuse => include_bytes!("../assets/icons/brand-muse.svg"),
            Self::BrandOpenCode => include_bytes!("../assets/icons/brand-opencode.svg"),
            Self::BrandQwen => include_bytes!("../assets/icons/brand-qwen.svg"),
            Self::BrandKimi => include_bytes!("../assets/icons/brand-kimi.svg"),
            Self::BrandKiro => include_bytes!("../assets/icons/brand-kiro.svg"),
            Self::BrandDevin => include_bytes!("../assets/icons/brand-devin.svg"),
            Self::BrandCopilot => include_bytes!("../assets/icons/brand-copilot.svg"),
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::PathBuf};

    use super::*;

    /// Extensions the loaders installed by `install_image_loaders` decode.
    /// `egui_extras` is pinned to `default-features = false, features =
    /// ["svg"]`, so its `ImageCrateLoader` is absent and the `SvgLoader`
    /// refuses any other extension: a raster asset here would silently
    /// render as egui's load-error placeholder, which no compiler catches.
    const DECODABLE_EXTENSIONS: [&str; 1] = ["svg"];

    fn asset_directory() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("icons")
    }

    /// The `bytes://` URI always uses forward slashes, whatever the host.
    fn asset_name(icon: Icon) -> &'static str {
        icon.uri()
            .rsplit('/')
            .next()
            .expect("an icon URI ends with its file name")
    }

    #[test]
    fn every_icon_asset_exists_and_an_installed_loader_decodes_it() {
        for icon in Icon::ALL {
            let name = asset_name(*icon);
            let extension = name
                .rsplit_once('.')
                .map(|(_, extension)| extension)
                .unwrap_or_default();
            assert!(
                DECODABLE_EXTENSIONS.contains(&extension),
                "{name} is a .{extension}, which no installed image loader decodes"
            );
            let path = asset_directory().join(name);
            assert!(path.is_file(), "{} is missing", path.display());
            // Comparing bytes rather than text keeps this line-ending
            // agnostic on a CRLF checkout.
            assert_eq!(
                fs::read(&path).expect("the icon asset is readable"),
                icon.bytes(),
                "{name} on disk differs from the embedded copy"
            );
            assert!(
                icon.bytes().starts_with(b"<svg"),
                "{name} is not an SVG document"
            );
        }
    }

    /// An asset left in the folder but wired to no variant is dead weight
    /// nobody notices: the orphaned Qwen PNG and a `chat.svg` that outlived
    /// its caller were both found this way. The whole folder is checked, not
    /// just the brand marks.
    #[test]
    fn every_vendored_asset_belongs_to_an_icon() {
        let wired: BTreeSet<&str> = Icon::ALL.iter().map(|icon| asset_name(*icon)).collect();
        for entry in fs::read_dir(asset_directory()).expect("the icon folder is readable") {
            let name = entry.expect("a readable directory entry").file_name();
            let name = name.to_string_lossy();
            assert!(
                wired.contains(name.as_ref()),
                "{name} is vendored but no Icon variant points at it"
            );
        }
    }

    #[test]
    fn icon_uris_are_unique() {
        let uris: BTreeSet<&str> = Icon::ALL.iter().map(|icon| icon.uri()).collect();
        assert_eq!(uris.len(), Icon::ALL.len(), "two icons share a URI");
    }
}
