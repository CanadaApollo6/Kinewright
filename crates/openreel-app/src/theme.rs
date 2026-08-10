use std::{collections::BTreeMap, sync::Arc};

use eframe::egui::style::WidgetVisuals;
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, Visuals,
};

pub(crate) mod color {
    use eframe::egui::Color32;

    pub const CANVAS: Color32 = Color32::from_rgb(0x0A, 0x0D, 0x11);
    pub const PANEL: Color32 = Color32::from_rgb(0x10, 0x14, 0x1A);
    pub const SURFACE: Color32 = Color32::from_rgb(0x16, 0x1B, 0x22);
    pub const SURFACE_RAISED: Color32 = Color32::from_rgb(0x1C, 0x22, 0x2B);
    pub const SURFACE_ACTIVE: Color32 = Color32::from_rgb(0x22, 0x2A, 0x34);
    pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(0x25, 0x2C, 0x36);
    pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x3A, 0x45, 0x53);
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xE6, 0xEC, 0xF2);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xA5, 0xAF, 0xBB);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x70, 0x7B, 0x88);
    pub const MEDIA_SHADOW: Color32 = Color32::from_rgb(0x05, 0x07, 0x0A);
    pub const ACCENT: Color32 = Color32::from_rgb(0x42, 0xC7, 0xC9);
    pub const STATUS_SUCCESS: Color32 = Color32::from_rgb(0x70, 0xC3, 0x91);
    pub const STATUS_WARNING: Color32 = Color32::from_rgb(0xD7, 0xB2, 0x6D);
    pub const STATUS_DANGER: Color32 = Color32::from_rgb(0xF0, 0x6C, 0x75);

    pub const ACCENT_72: Color32 = Color32::from_rgba_unmultiplied_const(0x42, 0xC7, 0xC9, 184);
    pub const ACCENT_28: Color32 = Color32::from_rgba_unmultiplied_const(0x42, 0xC7, 0xC9, 71);
    pub const ACCENT_16: Color32 = Color32::from_rgba_unmultiplied_const(0x42, 0xC7, 0xC9, 41);
    #[allow(dead_code)]
    pub const ACCENT_10: Color32 = Color32::from_rgba_unmultiplied_const(0x42, 0xC7, 0xC9, 26);
    pub const MEDIA_TINT_78: Color32 = Color32::from_rgba_unmultiplied_const(0xFF, 0xFF, 0xFF, 199);
    #[allow(dead_code)]
    pub const MEDIA_VEIL_24: Color32 = Color32::from_rgba_unmultiplied_const(0x05, 0x07, 0x0A, 61);
    pub const MEDIA_SCRIM_78: Color32 =
        Color32::from_rgba_unmultiplied_const(0x05, 0x07, 0x0A, 199);
    pub const TEXT_PRIMARY_64: Color32 =
        Color32::from_rgba_unmultiplied_const(0xE6, 0xEC, 0xF2, 163);
}

pub(crate) mod space {
    pub const HALF: f32 = 2.0;
    pub const ONE: f32 = 4.0;
    pub const ONE_HALF: f32 = 6.0;
    pub const TWO: f32 = 8.0;
    pub const THREE: f32 = 12.0;
    pub const FOUR: f32 = 16.0;
    pub const SIX: f32 = 24.0;
    pub const EIGHT: f32 = 32.0;
}

pub(crate) mod radius {
    use eframe::egui::CornerRadius;

    pub const NONE: CornerRadius = CornerRadius::ZERO;
    pub const XS: CornerRadius = CornerRadius::same(2);
    pub const SM: CornerRadius = CornerRadius::same(4);
    pub const MD: CornerRadius = CornerRadius::same(6);
    pub const LG: CornerRadius = CornerRadius::same(8);
}

pub(crate) mod size {
    pub const WINDOW_WIDTH: f32 = 1_440.0;
    pub const WINDOW_HEIGHT: f32 = 900.0;
    pub const WINDOW_MIN_WIDTH: f32 = 1_100.0;
    pub const WINDOW_MIN_HEIGHT: f32 = 700.0;
    pub const CONTROL_HEIGHT: f32 = 26.0;
    pub const ICON_SM: f32 = 14.0;
    pub const ICON_MD: f32 = 16.0;
    pub const ICON_LG: f32 = 18.0;
    pub const ICON_BUTTON: f32 = 26.0;
    pub const TRANSPORT_BUTTON: f32 = 30.0;
    pub const TOP_BAR_HEIGHT: f32 = 34.0;
    pub const TRANSPORT_HEIGHT: f32 = 34.0;
    pub const TIMELINE_TOOLBAR_HEIGHT: f32 = 32.0;
    pub const RULER_HEIGHT: f32 = 24.0;
    pub const TRACK_HEIGHT: f32 = 72.0;
}

pub(crate) mod motion {
    #[allow(dead_code)]
    pub const FAST: f32 = 0.080;
    pub const STANDARD: f32 = 0.140;
    pub const NAVIGATION: f32 = 0.180;
}

pub(crate) mod type_size {
    pub const TITLE: f32 = 18.0;
    pub const HEADING: f32 = 14.0;
    pub const BODY: f32 = 12.0;
    pub const CAPTION: f32 = 10.0;
    pub const MICRO: f32 = 9.0;
    pub const TIMECODE: f32 = 13.0;
    pub const RULER: f32 = 9.0;
    pub const CODE: f32 = 10.0;
}

pub(crate) fn title_font() -> FontId {
    FontId::new(type_size::TITLE, FontFamily::Proportional)
}

pub(crate) fn timecode_font() -> FontId {
    FontId::new(type_size::TIMECODE, FontFamily::Monospace)
}

pub(crate) fn ruler_font() -> FontId {
    FontId::new(type_size::RULER, FontFamily::Monospace)
}

pub(crate) fn code_font() -> FontId {
    FontId::new(type_size::CODE, FontFamily::Monospace)
}

// Design-system spacing tokens are small whole-point values accepted by egui as i8 margins.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn margin(points: f32) -> i8 {
    points as i8
}

pub(crate) fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(color::PANEL)
        .inner_margin(egui::Margin::same(margin(space::THREE)))
}

pub(crate) fn card_frame(selected: bool) -> egui::Frame {
    egui::Frame::new()
        .fill(if selected {
            color::ACCENT_28
        } else {
            color::SURFACE
        })
        .stroke(Stroke::new(
            1.0,
            if selected {
                color::ACCENT_72
            } else {
                color::BORDER_SUBTLE
            },
        ))
        .corner_radius(radius::MD)
        .inner_margin(egui::Margin::same(margin(space::TWO)))
}

pub(crate) fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.text_styles = BTreeMap::from([
        (
            TextStyle::Heading,
            FontId::new(type_size::HEADING, FontFamily::Proportional),
        ),
        (
            TextStyle::Body,
            FontId::new(type_size::BODY, FontFamily::Proportional),
        ),
        (
            TextStyle::Button,
            FontId::new(type_size::BODY, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(type_size::CAPTION, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(type_size::CODE, FontFamily::Monospace),
        ),
    ]);
    style.spacing.item_spacing = egui::vec2(space::TWO, space::ONE_HALF);
    style.spacing.window_margin = egui::Margin::same(margin(space::THREE));
    style.spacing.menu_margin = egui::Margin::same(margin(space::TWO));
    style.spacing.button_padding = egui::vec2(space::ONE_HALF, space::ONE);
    style.spacing.indent = space::FOUR;
    style.spacing.interact_size = egui::vec2(size::CONTROL_HEIGHT, size::CONTROL_HEIGHT);
    style.spacing.slider_width = 112.0;
    style.spacing.slider_rail_height = 3.0;
    style.spacing.combo_width = 120.0;
    style.spacing.text_edit_width = 180.0;
    style.spacing.icon_width = size::ICON_MD;
    style.spacing.icon_width_inner = size::ICON_SM;
    style.spacing.icon_spacing = space::ONE_HALF;
    style.animation_time = motion::STANDARD;
    style.compact_menu_style = true;
    style.visuals = visuals();
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_theme(egui::Theme::Dark);
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "OpenReel Inter".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Variable.ttf"
        ))),
    );
    fonts.font_data.insert(
        "OpenReel JetBrains Mono".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/JetBrainsMono-Variable.ttf"
        ))),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "OpenReel Inter".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "OpenReel JetBrains Mono".to_owned());
    ctx.set_fonts(fonts);
}

fn visuals() -> Visuals {
    let mut visuals = Visuals::dark();
    visuals.override_text_color = Some(color::TEXT_PRIMARY);
    visuals.weak_text_color = Some(color::TEXT_SECONDARY);
    visuals.selection.bg_fill = color::ACCENT_28;
    visuals.selection.stroke = Stroke::new(1.0, color::TEXT_PRIMARY);
    visuals.hyperlink_color = color::ACCENT;
    visuals.faint_bg_color = color::SURFACE;
    visuals.extreme_bg_color = color::CANVAS;
    visuals.text_edit_bg_color = Some(color::SURFACE);
    visuals.code_bg_color = color::CANVAS;
    visuals.warn_fg_color = color::STATUS_WARNING;
    visuals.error_fg_color = color::STATUS_DANGER;
    visuals.window_corner_radius = radius::LG;
    visuals.window_shadow = Shadow {
        offset: [0, 10],
        blur: 30,
        spread: 0,
        color: Color32::from_black_alpha(163),
    };
    visuals.window_fill = color::SURFACE_RAISED;
    visuals.window_stroke = Stroke::new(1.0, color::BORDER_STRONG);
    visuals.menu_corner_radius = radius::MD;
    visuals.panel_fill = color::PANEL;
    visuals.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(122),
    };
    visuals.button_frame = true;
    visuals.collapsing_header_frame = false;
    visuals.indent_has_left_vline = true;
    visuals.striped = false;
    visuals.slider_trailing_fill = true;
    visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
    visuals.image_loading_spinners = false;
    visuals.disabled_alpha = 0.55;
    visuals.widgets.noninteractive = widget(
        color::PANEL,
        Color32::TRANSPARENT,
        color::BORDER_SUBTLE,
        color::TEXT_SECONDARY,
        radius::SM,
    );
    visuals.widgets.inactive = widget(
        color::SURFACE,
        color::SURFACE,
        color::BORDER_STRONG,
        color::TEXT_SECONDARY,
        radius::SM,
    );
    visuals.widgets.hovered = widget(
        color::SURFACE_RAISED,
        color::SURFACE_RAISED,
        color::ACCENT_72,
        color::TEXT_PRIMARY,
        radius::SM,
    );
    visuals.widgets.active = widget(
        color::SURFACE_ACTIVE,
        color::SURFACE_ACTIVE,
        color::ACCENT,
        color::TEXT_PRIMARY,
        radius::SM,
    );
    visuals.widgets.open = visuals.widgets.active;
    visuals
}

fn widget(
    bg_fill: Color32,
    weak_bg_fill: Color32,
    border: Color32,
    foreground: Color32,
    corner_radius: CornerRadius,
) -> WidgetVisuals {
    WidgetVisuals {
        bg_fill,
        weak_bg_fill,
        bg_stroke: Stroke::new(1.0, border),
        corner_radius,
        fg_stroke: Stroke::new(1.0, foreground),
        expansion: 0.0,
    }
}
