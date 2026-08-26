use std::{collections::BTreeMap, sync::Arc};

use eframe::egui::style::WidgetVisuals;
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, Visuals,
};

pub(crate) mod color {
    use eframe::egui::Color32;

    pub const CANVAS: Color32 = Color32::from_rgb(0x13, 0x15, 0x19);
    pub const PANEL: Color32 = Color32::from_rgb(0x1A, 0x1D, 0x23);
    pub const SURFACE: Color32 = Color32::from_rgb(0x21, 0x25, 0x2C);
    pub const SURFACE_RAISED: Color32 = Color32::from_rgb(0x27, 0x2C, 0x34);
    pub const SURFACE_ACTIVE: Color32 = Color32::from_rgb(0x2E, 0x34, 0x3D);
    pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(0x35, 0x3C, 0x46);
    pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x46, 0x50, 0x5C);
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xE6, 0xEC, 0xF2);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xA8, 0xB0, 0xBA);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x78, 0x81, 0x8C);
    /// Viewer letterbox and timeline track well: darker than chrome CANVAS.
    pub const LETTERBOX: Color32 = Color32::from_rgb(0x0B, 0x0C, 0x0E);
    pub const MEDIA_SHADOW: Color32 = LETTERBOX;
    pub const ACCENT: Color32 = Color32::from_rgb(0x42, 0xC7, 0xC9);
    pub const ACCENT_DIM_BORDER: Color32 = Color32::from_rgb(0x1E, 0x4E, 0x50);
    pub const STATUS_SUCCESS: Color32 = Color32::from_rgb(0x70, 0xC3, 0x91);
    pub const STATUS_WARNING: Color32 = Color32::from_rgb(0xD7, 0xB2, 0x6D);
    pub const STATUS_DANGER: Color32 = Color32::from_rgb(0xF0, 0x6C, 0x75);

    /// Accent at 14% alpha: persistent selection and demoted accent fills.
    pub const ACCENT_WASH: Color32 = Color32::from_rgba_unmultiplied_const(0x42, 0xC7, 0xC9, 36);
    pub const MEDIA_TINT_78: Color32 = Color32::from_rgba_unmultiplied_const(0xFF, 0xFF, 0xFF, 199);
    #[allow(dead_code)]
    pub const MEDIA_VEIL_24: Color32 = Color32::from_rgba_unmultiplied_const(0x0B, 0x0C, 0x0E, 61);
    pub const MEDIA_SCRIM_78: Color32 =
        Color32::from_rgba_unmultiplied_const(0x0B, 0x0C, 0x0E, 199);
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
    /// Modals and windows. Menus use [`MD`].
    pub const LG: CornerRadius = CornerRadius::same(8);

    #[must_use]
    pub(crate) fn px(corner: CornerRadius) -> f32 {
        f32::from(corner.nw)
    }
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

// One step larger across the scale (Riel: everything read a bit small);
// body lands between Zed's small (12) and default (14).
pub(crate) mod type_size {
    pub const TITLE: f32 = 19.0;
    pub const HEADING: f32 = 15.0;
    pub const BODY: f32 = 13.0;
    pub const CAPTION: f32 = 11.0;
    pub const MICRO: f32 = 10.0;
    pub const TIMECODE: f32 = 14.0;
    pub const RULER: f32 = 10.0;
    pub const CODE: f32 = 11.0;
}

const INTER_MEDIUM: &str = "InterMedium";
const INTER_SEMIBOLD: &str = "InterSemiBold";

pub(crate) fn timecode_font() -> FontId {
    FontId::new(type_size::TIMECODE, FontFamily::Monospace)
}

pub(crate) fn ruler_font() -> FontId {
    FontId::new(type_size::RULER, FontFamily::Monospace)
}

pub(crate) fn code_font() -> FontId {
    FontId::new(type_size::CODE, FontFamily::Monospace)
}

#[must_use]
pub(crate) fn medium(size: f32) -> egui::FontId {
    FontId::new(size, FontFamily::Name(INTER_MEDIUM.into()))
}

#[must_use]
pub(crate) fn semibold(size: f32) -> egui::FontId {
    FontId::new(size, FontFamily::Name(INTER_SEMIBOLD.into()))
}

/// Small-caps section label: micro size, letter-spaced, muted by default.
#[must_use]
pub(crate) fn caps_label(text: &str, color: egui::Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: medium(type_size::MICRO),
            color,
            extra_letter_spacing: 1.2,
            ..Default::default()
        },
    );
    job
}

/// Caps prefix plus an untracked remainder, for labels like `TOOL · name`.
#[must_use]
pub(crate) fn caps_prefix(prefix: &str, rest: &str, color: egui::Color32) -> egui::text::LayoutJob {
    let mut job = caps_label(prefix, color);
    if rest.is_empty() {
        return job;
    }
    job.append(
        rest,
        0.0,
        egui::TextFormat {
            font_id: medium(type_size::MICRO),
            color,
            extra_letter_spacing: 0.0,
            ..Default::default()
        },
    );
    job
}

/// Product wordmark: semibold title with tracked caps.
#[must_use]
pub(crate) fn wordmark(text: &str, color: egui::Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: semibold(type_size::TITLE),
            color,
            extra_letter_spacing: 1.6,
            ..Default::default()
        },
    );
    job
}

/// Paint a tracked caps label with the same anchors as [`egui::Painter::text`].
pub(crate) fn paint_caps(
    painter: &egui::Painter,
    pos: egui::Pos2,
    anchor: egui::Align2,
    text: &str,
    color: egui::Color32,
) {
    let galley = painter.layout_job(caps_label(text, color));
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
}

/// Paint the house lighting on a raised surface: a faint sheen falling
/// from the top plus a 1px specular top edge. Call AFTER the frame's
/// contents so it overlays the fill (alphas are single-digit; content
/// remains fully legible).
pub(crate) fn paint_raised_lighting(painter: &egui::Painter, rect: egui::Rect, corner: f32) {
    if !rect.is_positive() {
        return;
    }
    let painter = painter.with_clip_rect(rect);
    let sheen_height = (rect.height() * 0.5).min(48.0);
    let sheen = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), sheen_height));
    let mut mesh = egui::Mesh::default();
    // Alphas settled by screenshot review: the spec's 5/14 vanished in a
    // real capture; these are the collaborating designer's counter-values.
    let top = egui::Color32::from_white_alpha(8);
    let bottom = egui::Color32::TRANSPARENT;
    mesh.colored_vertex(sheen.left_top(), top);
    mesh.colored_vertex(sheen.right_top(), top);
    mesh.colored_vertex(sheen.right_bottom(), bottom);
    mesh.colored_vertex(sheen.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
    painter.hline(
        egui::Rangef::new(rect.left() + corner, rect.right() - corner),
        rect.top() + 0.5,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(20)),
    );
}

/// Darken the top of an inset well so it reads as recessed (light from above).
pub(crate) fn paint_inset_well(painter: &egui::Painter, rect: egui::Rect, corner: f32) {
    if !rect.is_positive() {
        return;
    }
    let painter = painter.with_clip_rect(rect);
    let shade_height = 6.0_f32.min(rect.height() * 0.25);
    let shade = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), shade_height));
    let mut mesh = egui::Mesh::default();
    let top = egui::Color32::from_black_alpha(18);
    let bottom = egui::Color32::TRANSPARENT;
    mesh.colored_vertex(shade.left_top(), top);
    mesh.colored_vertex(shade.right_top(), top);
    mesh.colored_vertex(shade.right_bottom(), bottom);
    mesh.colored_vertex(shade.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
    painter.hline(
        egui::Rangef::new(rect.left() + corner, rect.right() - corner),
        rect.top() + 0.5,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(40)),
    );
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
            color::ACCENT_WASH
        } else {
            color::SURFACE
        })
        .stroke(Stroke::new(
            if selected { 1.0 } else { 0.0 },
            color::ACCENT_DIM_BORDER,
        ))
        .corner_radius(radius::MD)
        .inner_margin(egui::Margin::same(margin(space::TWO)))
}

/// Input outline: strong rest edge, full accent only while focused.
#[must_use]
pub(crate) fn input_stroke(focused: bool) -> Stroke {
    Stroke::new(
        1.0,
        if focused {
            color::ACCENT
        } else {
            color::BORDER_STRONG
        },
    )
}

/// Scope TextEdit/Drag-adjacent fields so they pick up input outlines
/// without giving every button a ring.
pub(crate) fn apply_input_visuals(ui: &mut egui::Ui) {
    let rest = input_stroke(false);
    let focus = input_stroke(true);
    let visuals = ui.visuals_mut();
    visuals.widgets.inactive.bg_stroke = rest;
    visuals.widgets.hovered.bg_stroke = focus;
    visuals.widgets.active.bg_stroke = focus;
}

pub(crate) fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.text_styles = BTreeMap::from([
        (TextStyle::Heading, semibold(type_size::HEADING)),
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
    // Wider horizontal padding gives borderless buttons their pill
    // proportions; hierarchy comes from fills, not outlines (M25).
    style.spacing.button_padding = egui::vec2(space::TWO, space::ONE);
    style.spacing.scroll = egui::style::ScrollStyle::thin();
    style.spacing.indent = space::FOUR;
    style.spacing.interact_size = egui::vec2(size::CONTROL_HEIGHT, size::CONTROL_HEIGHT);
    style.spacing.slider_width = 112.0;
    style.spacing.slider_rail_height = 3.0;
    // Combos size to their content; a wide floor mostly buys dead air and
    // overflows narrow columns (the composer row wraps, but later).
    style.spacing.combo_width = 76.0;
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
        "Kinewright Inter".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Variable.ttf"
        ))),
    );
    fonts.font_data.insert(
        INTER_MEDIUM.to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Medium.ttf"
        ))),
    );
    fonts.font_data.insert(
        INTER_SEMIBOLD.to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    fonts.font_data.insert(
        "Kinewright JetBrains Mono".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/JetBrainsMono-Variable.ttf"
        ))),
    );
    let proportional_fallbacks = fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .clone();
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Kinewright Inter".to_owned());
    fonts.families.insert(
        FontFamily::Name(INTER_MEDIUM.into()),
        prepend_family(INTER_MEDIUM, &proportional_fallbacks),
    );
    fonts.families.insert(
        FontFamily::Name(INTER_SEMIBOLD.into()),
        prepend_family(INTER_SEMIBOLD, &proportional_fallbacks),
    );
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "Kinewright JetBrains Mono".to_owned());
    ctx.set_fonts(fonts);
}

fn prepend_family(primary: &str, fallbacks: &[String]) -> Vec<String> {
    let mut family = Vec::with_capacity(fallbacks.len().saturating_add(1));
    family.push(primary.to_owned());
    family.extend(fallbacks.iter().cloned());
    family
}

fn visuals() -> Visuals {
    let mut visuals = Visuals::dark();
    visuals.override_text_color = Some(color::TEXT_PRIMARY);
    visuals.weak_text_color = Some(color::TEXT_SECONDARY);
    visuals.selection.bg_fill = color::ACCENT_WASH;
    visuals.selection.stroke = Stroke::new(1.0, color::ACCENT);
    visuals.hyperlink_color = color::ACCENT;
    visuals.faint_bg_color = color::SURFACE;
    visuals.extreme_bg_color = color::CANVAS;
    visuals.text_edit_bg_color = Some(color::SURFACE);
    visuals.code_bg_color = color::CANVAS;
    visuals.warn_fg_color = color::STATUS_WARNING;
    visuals.error_fg_color = color::STATUS_DANGER;
    visuals.window_corner_radius = radius::LG;
    visuals.window_shadow = Shadow {
        offset: [0, 6],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(102),
    };
    visuals.window_fill = color::SURFACE_ACTIVE;
    visuals.window_stroke = Stroke::new(1.0, color::BORDER_STRONG);
    visuals.menu_corner_radius = radius::MD;
    visuals.panel_fill = color::PANEL;
    visuals.popup_shadow = Shadow {
        offset: [0, 3],
        blur: 8,
        spread: 0,
        color: Color32::from_black_alpha(82),
    };
    visuals.button_frame = true;
    visuals.collapsing_header_frame = false;
    visuals.indent_has_left_vline = true;
    visuals.striped = false;
    visuals.slider_trailing_fill = true;
    visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
    visuals.image_loading_spinners = false;
    visuals.disabled_alpha = 0.55;
    // Border discipline (M25, Zed as the bar): widget hierarchy is carried
    // by the surface ladder and text weight, never by outlines. Hairlines
    // stay on noninteractive chrome (separators) and true containers;
    // interaction states step up the fill ladder instead of growing rings.
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
        Color32::TRANSPARENT,
        color::TEXT_SECONDARY,
        radius::SM,
    );
    visuals.widgets.hovered = widget(
        color::SURFACE_RAISED,
        color::SURFACE_RAISED,
        Color32::TRANSPARENT,
        color::TEXT_PRIMARY,
        radius::SM,
    );
    visuals.widgets.active = widget(
        color::SURFACE_ACTIVE,
        color::SURFACE_ACTIVE,
        Color32::TRANSPARENT,
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

/// Every string one painted frame emitted, in paint order.
///
/// Test support. egui keeps no retained widget tree, so the only way to prove
/// a surface *said* something is to read the text shapes it painted. Shared
/// here rather than transcribed per module so a headless assertion means the
/// same thing everywhere.
#[cfg(test)]
#[must_use]
pub(crate) fn painted_text(output: &egui::FullOutput) -> Vec<String> {
    fn collect(shape: &egui::epaint::Shape, into: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Text(text) => into.push(text.galley.text().to_owned()),
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, into);
                }
            }
            _ => {}
        }
    }

    let mut text = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut text);
    }
    text
}
