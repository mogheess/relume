//! Relume design system: palettes (dark first), Inter typography, Phosphor icons, and the
//! custom widgets every screen is built from.

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    IconData, Painter, Rect, Response, RichText, Sense, Shadow, Stroke, StrokeKind, TextStyle, TextureHandle, TextureOptions, Ui,
    Vec2,
};
pub use egui_phosphor::regular as ic;

use relume_core::model::{FileState, Health};

pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub raised: Color32,
    pub field: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub photo: Color32,
    pub video: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub bad: Color32,
    pub map_free: Color32,
    pub map_used: Color32,
    pub map_read: Color32,
}

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xF5, 0xF6, 0xF8),
    surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    raised: Color32::from_rgb(0xF1, 0xF3, 0xF6),
    field: Color32::from_rgb(0xEC, 0xEE, 0xF2),
    border: Color32::from_rgb(0xE2, 0xE5, 0xEA),
    text: Color32::from_rgb(0x10, 0x13, 0x18),
    muted: Color32::from_rgb(0x5B, 0x62, 0x70),
    faint: Color32::from_rgb(0x98, 0x9F, 0xAB),
    accent: Color32::from_rgb(0x25, 0x63, 0xEB),
    accent_hover: Color32::from_rgb(0x1D, 0x4E, 0xD8),
    accent_soft: Color32::from_rgb(0xE8, 0xEF, 0xFD),
    on_accent: Color32::WHITE,
    photo: Color32::from_rgb(0x08, 0x91, 0xB2),
    video: Color32::from_rgb(0xEA, 0x58, 0x0C),
    good: Color32::from_rgb(0x16, 0xA3, 0x4A),
    warn: Color32::from_rgb(0xCA, 0x8A, 0x04),
    bad: Color32::from_rgb(0xDC, 0x26, 0x26),
    map_free: Color32::from_rgb(0xEB, 0xED, 0xF1),
    map_used: Color32::from_rgb(0xC7, 0xCC, 0xD5),
    map_read: Color32::from_rgb(0xC9, 0xD8, 0xFB),
};

pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x0F, 0x11, 0x15),
    surface: Color32::from_rgb(0x16, 0x19, 0x1F),
    raised: Color32::from_rgb(0x1D, 0x21, 0x28),
    field: Color32::from_rgb(0x24, 0x28, 0x30),
    border: Color32::from_rgb(0x2C, 0x31, 0x3A),
    text: Color32::from_rgb(0xEC, 0xEE, 0xF2),
    muted: Color32::from_rgb(0x9A, 0xA1, 0xAD),
    faint: Color32::from_rgb(0x64, 0x6B, 0x78),
    accent: Color32::from_rgb(0x3B, 0x82, 0xF6),
    accent_hover: Color32::from_rgb(0x60, 0x9A, 0xF8),
    accent_soft: Color32::from_rgb(0x1A, 0x2A, 0x47),
    on_accent: Color32::WHITE,
    photo: Color32::from_rgb(0x22, 0xB8, 0xD6),
    video: Color32::from_rgb(0xFB, 0x92, 0x3C),
    good: Color32::from_rgb(0x22, 0xC5, 0x5E),
    warn: Color32::from_rgb(0xEA, 0xB3, 0x08),
    bad: Color32::from_rgb(0xEF, 0x44, 0x44),
    map_free: Color32::from_rgb(0x22, 0x26, 0x2E),
    map_used: Color32::from_rgb(0x3A, 0x40, 0x4B),
    map_read: Color32::from_rgb(0x1F, 0x3A, 0x6B),
};

pub fn pal(ui: &Ui) -> &'static Palette {
    if ui.visuals().dark_mode { &DARK } else { &LIGHT }
}

#[derive(Clone, Copy)]
pub enum W {
    Regular,
    Medium,
    Semibold,
}

pub fn font(size: f32, w: W) -> FontId {
    match w {
        W::Regular => FontId::proportional(size),
        W::Medium => FontId::new(size, FontFamily::Name("medium".into())),
        W::Semibold => FontId::new(size, FontFamily::Name("semibold".into())),
    }
}

pub fn txt(s: impl Into<String>, size: f32, w: W, color: Color32) -> RichText {
    RichText::new(s.into()).font(font(size, w)).color(color)
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let inter: &'static [u8] = damascene_fonts_inter::INTER_VARIABLE;
    let mk = |wght: f32| {
        FontData::from_static(inter).tweak(egui::FontTweak {
            coords: egui::epaint::text::VariationCoords::new([("wght", wght)]),
            ..Default::default()
        })
    };
    fonts.font_data.insert("inter".into(), mk(430.0).into());
    fonts.font_data.insert("inter-medium".into(), mk(540.0).into());
    fonts.font_data.insert("inter-semibold".into(), mk(640.0).into());
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    // Icons get their own family: Inter defines some of the same private-use codepoints.
    fonts.font_data.insert("phosphor".into(), FontData::from_static(egui_phosphor::Variant::Regular.font_bytes()).into());
    fonts.families.insert(FontFamily::Name("icons".into()), vec!["phosphor".into(), "inter".into()]);
    let base: Vec<String> = fonts.families[&FontFamily::Proportional].iter().skip(1).cloned().collect();
    for (fam, first) in [("medium", "inter-medium"), ("semibold", "inter-semibold")] {
        let mut v = vec![first.to_string()];
        v.extend(base.iter().cloned());
        fonts.families.insert(FontFamily::Name(fam.into()), v);
    }
    ctx.set_fonts(fonts);
}

pub fn setup(ctx: &egui::Context) {
    install_fonts(ctx);
    for dark in [true, false] {
        let p = if dark { &DARK } else { &LIGHT };
        let theme = if dark { egui::Theme::Dark } else { egui::Theme::Light };
        ctx.style_mut_of(theme, |s| {
            s.text_styles.insert(TextStyle::Heading, font(26.0, W::Semibold));
            s.text_styles.insert(TextStyle::Body, font(14.0, W::Regular));
            s.text_styles.insert(TextStyle::Button, font(14.0, W::Medium));
            s.text_styles.insert(TextStyle::Small, font(12.5, W::Regular));
            s.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.0));
            s.spacing.item_spacing = vec2(8.0, 8.0);
            s.spacing.button_padding = vec2(12.0, 7.0);
            s.spacing.interact_size.y = 30.0;
            s.spacing.menu_margin = egui::Margin::same(8);
            s.spacing.window_margin = egui::Margin::same(20);
            s.spacing.scroll.bar_width = 8.0;
            s.spacing.scroll.floating = true;
            s.spacing.slider_rail_height = 4.0;
            let v = &mut s.visuals;
            v.dark_mode = dark;
            v.panel_fill = p.bg;
            v.window_fill = p.surface;
            v.extreme_bg_color = p.field;
            v.faint_bg_color = p.raised;
            v.code_bg_color = p.field;
            v.window_stroke = Stroke::new(1.0, p.border);
            v.window_corner_radius = CornerRadius::same(16);
            v.menu_corner_radius = CornerRadius::same(12);
            v.window_shadow = Shadow { offset: [0, 12], blur: 40, spread: 0, color: Color32::from_black_alpha(if dark { 140 } else { 40 }) };
            v.popup_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if dark { 120 } else { 30 }) };
            v.selection.bg_fill = p.accent_soft;
            v.selection.stroke = Stroke::new(1.0, p.accent);
            v.hyperlink_color = p.accent;
            v.override_text_color = Some(p.text);
            v.slider_trailing_fill = true;
            v.handle_shape = egui::style::HandleShape::Circle;
            let r = CornerRadius::same(9);
            for (w, fill, stroke) in [
                (&mut v.widgets.noninteractive, p.surface, p.border),
                (&mut v.widgets.inactive, p.field, Color32::TRANSPARENT),
                (&mut v.widgets.hovered, p.raised, p.border),
                (&mut v.widgets.active, p.raised, p.accent),
                (&mut v.widgets.open, p.raised, p.border),
            ] {
                w.bg_fill = fill;
                w.weak_bg_fill = fill;
                w.bg_stroke = Stroke::new(1.0, stroke);
                w.corner_radius = r;
                w.fg_stroke = Stroke::new(1.4, p.text);
            }
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.muted);
        });
    }
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

/// An icon as rich text (for `ui.label`).
pub fn ico(icon: &str, size: f32, color: Color32) -> RichText {
    RichText::new(icon).font(icon_font(size)).color(color)
}

/// Width of an icon + label pair as drawn by `paint_icon_label`.
pub fn icon_label_width(painter: &Painter, icon: &str, label: &str, f: &FontId) -> f32 {
    let lw = if label.is_empty() { 0.0 } else { painter.layout_no_wrap(label.to_string(), f.clone(), Color32::WHITE).size().x };
    let iw = if icon.is_empty() { 0.0 } else { f.size + 3.0 };
    let gap = if !icon.is_empty() && !label.is_empty() { 7.0 } else { 0.0 };
    iw + gap + lw
}

/// Draw an icon followed by a label, centered on `center`.
pub fn paint_icon_label(painter: &Painter, center: egui::Pos2, icon: &str, label: &str, f: &FontId, color: Color32) {
    let w = icon_label_width(painter, icon, label, f);
    let mut x = center.x - w / 2.0;
    if !icon.is_empty() {
        painter.text(pos2(x, center.y), Align2::LEFT_CENTER, icon, icon_font(f.size + 3.0), color);
        x += f.size + 3.0 + if label.is_empty() { 0.0 } else { 7.0 };
    }
    if !label.is_empty() {
        painter.text(pos2(x, center.y), Align2::LEFT_CENTER, label, f.clone(), color);
    }
}

fn hover_cursor(r: &Response) {
    if r.hovered() {
        r.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

/// Main call to action: solid accent, white text, optional icon.
pub fn primary_button(ui: &mut Ui, icon: &str, text: &str, enabled: bool) -> Response {
    let p = pal(ui);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font(14.5, W::Semibold), p.on_accent);
    let icon_w = if icon.is_empty() { 0.0 } else { 26.0 };
    let size = vec2(galley.size().x + icon_w + 40.0, 40.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let fill = if !enabled {
            p.field
        } else if resp.hovered() || resp.is_pointer_button_down_on() {
            p.accent_hover
        } else {
            p.accent
        };
        ui.painter().rect_filled(rect, 10.0, fill);
        let mut x = rect.left() + 20.0;
        let col = if enabled { p.on_accent } else { p.muted };
        if !icon.is_empty() {
            ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, icon, icon_font(18.0), col);
            x += icon_w;
        }
        ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, col);
    }
    if enabled {
        hover_cursor(&resp);
    }
    resp
}

/// Quiet button with optional icon.
pub fn ghost_button(ui: &mut Ui, icon: &str, text: &str) -> Response {
    let p = pal(ui);
    let f = font(14.0, W::Medium);
    let w = icon_label_width(ui.painter(), icon, text, &f) + 28.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w.max(36.0), 36.0), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        p.field
    } else if resp.hovered() {
        lerp(p.raised, p.field, 0.5)
    } else {
        p.raised
    };
    ui.painter().rect(rect, 10.0, fill, Stroke::new(1.0, p.border), StrokeKind::Inside);
    paint_icon_label(ui.painter(), rect.center(), icon, text, &f, p.text);
    hover_cursor(&resp);
    resp
}

/// Square icon-only button, highlighted when `active`.
pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str, active: bool) -> Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
    let bg = if active {
        p.accent_soft
    } else if resp.hovered() {
        p.raised
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 10.0, bg);
    let c = if active {
        p.accent
    } else if resp.hovered() {
        p.text
    } else {
        p.muted
    };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, icon_font(19.0), c);
    hover_cursor(&resp);
    resp.on_hover_text(tip)
}

/// Pill segmented control. Returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, id: &str, value: &mut T, items: &[(T, &str, &str)], icons_only: bool) -> bool {
    let p = pal(ui);
    let pad = 4.0;
    let h = 38.0;
    let f = font(13.5, W::Medium);
    let lab = |l: &'static str| -> &'static str { if icons_only { "" } else { l } };
    let widths: Vec<f32> = items.iter().map(|(_, icon, label)| icon_label_width(ui.painter(), icon, if icons_only && !icon.is_empty() { "" } else { label }, &f) + 28.0).collect();
    let _ = lab;
    let total: f32 = widths.iter().sum::<f32>() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, h), Sense::hover());
    ui.painter().rect_filled(rect, 12.0, p.field);
    let mut x = rect.left() + pad;
    let mut changed = false;
    for (i, ((v, icon, label), w)) in items.iter().zip(widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top() + pad), vec2(w, h - pad * 2.0));
        let resp = ui.interact(r, ui.id().with((id, i)), Sense::click());
        let sel = *value == *v;
        if sel {
            ui.painter().rect_filled(r, 9.0, p.surface);
            ui.painter().rect_stroke(r, 9.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 9.0, p.raised);
        }
        let col = if sel { p.text } else { p.muted };
        paint_icon_label(ui.painter(), r.center(), icon, if icons_only && !icon.is_empty() { "" } else { label }, &f, col);
        if resp.clicked() && !sel {
            *value = *v;
            changed = true;
        }
        hover_cursor(&resp);
        x += w;
    }
    changed
}

/// Toggleable pill with an icon (e.g. Photos / Videos).
pub fn toggle_chip(ui: &mut Ui, on: &mut bool, icon: &str, label: &str, color: Color32) -> Response {
    let p = pal(ui);
    let f = font(13.5, W::Medium);
    let (rect, resp) = ui.allocate_exact_size(vec2(icon_label_width(ui.painter(), icon, label, &f) + 32.0, 38.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let (fill, stroke) = if *on {
        (color.gamma_multiply(0.15), Stroke::new(1.3, color.gamma_multiply(0.75)))
    } else {
        (if resp.hovered() { p.raised } else { p.field }, Stroke::new(1.0, Color32::TRANSPARENT))
    };
    ui.painter().rect(rect, 19.0, fill, stroke, StrokeKind::Inside);
    paint_icon_label(ui.painter(), rect.center(), icon, label, &f, if *on { color } else { p.muted });
    hover_cursor(&resp);
    resp
}

/// Switch with a label.
pub fn switch(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let p = pal(ui);
    let g = ui.painter().layout_no_wrap(label.to_string(), font(13.5, W::Regular), p.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0 + 10.0 + g.size().x, 26.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let track = Rect::from_min_size(pos2(rect.left(), rect.center().y - 10.0), vec2(36.0, 20.0));
    if *on {
        ui.painter().rect_filled(track, 10.0, p.accent);
    } else {
        ui.painter().rect_filled(track, 10.0, p.field);
        ui.painter().rect_stroke(track, 10.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
    }
    let knob_x = track.left() + 10.0 + t * 16.0;
    ui.painter().circle_filled(pos2(knob_x, track.center().y), 7.5, Color32::WHITE);
    ui.painter().galley(pos2(track.right() + 10.0, rect.center().y - g.size().y / 2.0), g, p.text);
    hover_cursor(&resp);
    resp
}

/// Non-interactive pill tag.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) {
    let g = ui.painter().layout_no_wrap(text.to_string(), font(12.5, W::Medium), color);
    let (rect, _) = ui.allocate_exact_size(vec2(g.size().x + 18.0, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, 12.0, color.gamma_multiply(0.14));
    ui.painter().galley(pos2(rect.left() + 9.0, rect.center().y - g.size().y / 2.0), g, color);
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tri {
    Off,
    Some,
    All,
}

/// Painted checkbox with indeterminate state.
pub fn tri_check(ui: &mut Ui, state: Tri, size: f32) -> Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    paint_check(ui.painter(), rect, state, resp.hovered(), p);
    hover_cursor(&resp);
    resp
}

pub fn paint_check(painter: &Painter, rect: Rect, state: Tri, hovered: bool, p: &Palette) {
    let r = (rect.height() * 0.3) as u8;
    match state {
        Tri::Off => {
            painter.rect_filled(rect, r, if hovered { p.raised } else { Color32::TRANSPARENT });
            painter.rect_stroke(rect, r, Stroke::new(1.5, if hovered { p.accent } else { p.faint }), StrokeKind::Inside);
        }
        Tri::Some | Tri::All => {
            painter.rect_filled(rect, r, p.accent);
            let c = rect.center();
            let s = rect.width() * 0.24;
            let st = Stroke::new(2.0, Color32::WHITE);
            if state == Tri::All {
                painter.line_segment([pos2(c.x - s, c.y), pos2(c.x - s * 0.2, c.y + s * 0.8)], st);
                painter.line_segment([pos2(c.x - s * 0.2, c.y + s * 0.8), pos2(c.x + s * 1.1, c.y - s * 0.75)], st);
            } else {
                painter.line_segment([pos2(c.x - s, c.y), pos2(c.x + s, c.y)], st);
            }
        }
    }
}

/// Rounded tinted square holding an icon (drive, folder, file type).
pub fn icon_tile(ui: &mut Ui, icon: &str, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_icon_tile(ui.painter(), rect, icon, color);
}

pub fn paint_icon_tile(painter: &Painter, rect: Rect, icon: &str, color: Color32) {
    painter.rect_filled(rect, rect.width() * 0.28, color.gamma_multiply(0.15));
    painter.text(rect.center(), Align2::CENTER_CENTER, icon, icon_font(rect.width() * 0.5), color);
}

pub fn health_color(p: &Palette, h: Health) -> Color32 {
    match h {
        Health::Excellent | Health::Good => p.good,
        Health::Damaged => p.warn,
        Health::Overwritten => p.bad,
        Health::Unknown => p.faint,
    }
}

pub fn health_text(h: Health) -> &'static str {
    match h {
        Health::Excellent => "Excellent",
        Health::Good => "Good",
        Health::Damaged => "Damaged",
        Health::Overwritten => "Overwritten",
        Health::Unknown => "Unchecked",
    }
}

pub fn health_badge(ui: &mut Ui, h: Health) {
    let p = pal(ui);
    let c = health_color(p, h);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(vec2(8.0, 16.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, c);
        ui.label(txt(health_text(h), 13.5, W::Medium, c));
    });
}

pub fn state_text(s: FileState) -> &'static str {
    match s {
        FileState::Deleted => "Deleted",
        FileState::RecycleBin => "Recycle Bin",
        FileState::Lost => "Deep scan",
        FileState::Existing => "Not deleted",
    }
}

/// Clickable surface card with hover and selected states; `add` draws the content.
pub fn card(ui: &mut Ui, selected: bool, size: Vec2, add: impl FnOnce(&mut Ui, &Palette)) -> Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = resp.hovered();
    let fill = if selected {
        p.accent_soft
    } else if hovered {
        p.raised
    } else {
        p.surface
    };
    ui.painter().rect_filled(rect, 14.0, fill);
    let stroke = if selected {
        Stroke::new(1.6, p.accent)
    } else {
        Stroke::new(1.0, if hovered { lerp(p.border, p.faint, 0.5) } else { p.border })
    };
    ui.painter().rect_stroke(rect, 14.0, stroke, StrokeKind::Inside);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(16.0, 12.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    add(&mut child, p);
    hover_cursor(&resp);
    resp
}

pub fn usage_bar(ui: &mut Ui, used: f32, width: f32, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, 3.0, p.field);
    let mut r = rect;
    r.set_width((rect.width() * used.clamp(0.0, 1.0)).max(6.0));
    ui.painter().rect_filled(r, 3.0, if used > 0.92 { p.warn } else { p.accent });
}

/// Segmented circular gauge (ticks). `sweep` in 0..1 animates an indeterminate highlight.
pub fn ring_gauge(painter: &Painter, center: egui::Pos2, radius: f32, frac: f32, p: &Palette, sweep: Option<f64>) {
    let n = 60;
    let lit = (frac.clamp(0.0, 1.0) * n as f32).round() as usize;
    for i in 0..n {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 / n as f32 * std::f32::consts::TAU;
        let (s, c) = a.sin_cos();
        let r0 = radius - radius * 0.16;
        let p0 = center + vec2(c * r0, s * r0);
        let p1 = center + vec2(c * radius, s * radius);
        let swept = sweep.is_some_and(|sw| (i as f64 - sw * n as f64).rem_euclid(n as f64) < 5.0);
        let col = if i < lit {
            p.accent
        } else if swept {
            p.accent.gamma_multiply(0.5)
        } else {
            p.field
        };
        painter.line_segment([p0, p1], Stroke::new((radius * 0.06).max(2.5), col));
    }
}

pub fn section_title(ui: &mut Ui, title: &str) {
    let p = pal(ui);
    ui.label(txt(title, 16.0, W::Semibold, p.text));
}

/// Window / taskbar icon.
pub fn app_icon() -> IconData {
    let img = image::load_from_memory(include_bytes!("../windows/icon_128.png")).expect("icon").to_rgba8();
    IconData { width: img.width(), height: img.height(), rgba: img.into_raw() }
}

/// Brand mark texture for the top bar.
pub fn logo_texture(ctx: &egui::Context) -> TextureHandle {
    let img = image::load_from_memory(include_bytes!("../windows/icon_64.png")).expect("icon").to_rgba8();
    let ci = ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    ctx.load_texture("logo", ci, TextureOptions::LINEAR)
}

/// Shorten text with an ellipsis so it fits `max_w`.
pub fn ellipsize(ui: &Ui, s: &str, f: &FontId, max_w: f32) -> String {
    let fits = |t: String| ui.painter().layout_no_wrap(t, f.clone(), Color32::WHITE).size().x <= max_w;
    if fits(s.to_string()) {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(chars[..mid].iter().collect::<String>() + "…") {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}
