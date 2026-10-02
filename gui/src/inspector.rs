//! Details panel: preview and facts for the focused file, or a summary for a folder.

use eframe::egui::{self, pos2, vec2, Align2, Color32, Rect, Sense, Ui};

use relume_core::formats::Category;
use relume_core::model::{FoundFile, Health, Origin};
use relume_core::util::{format_date, format_duration, format_size};

use crate::app::App;
use crate::preview::Slot;
use crate::results::Focused;
use crate::theme::{self, ic, pal, txt, Palette, Tri, W};

pub fn show(app: &mut App, ui: &mut Ui, focus: &Focused) {
    let p = pal(ui);
    match focus {
        Focused::None => {
            ui.add_space(70.0);
            ui.vertical_centered(|ui| {
                ui.label(theme::ico(ic::IMAGES, 46.0, p.faint));
                ui.add_space(6.0);
                ui.label(txt("Select a file to preview it", 16.0, W::Semibold, p.text));
                ui.label(txt("Previews are read straight from the disk.", 14.0, W::Regular, p.muted));
            });
        }
        Focused::Dir { path, name, photos, videos, bytes, ids } => folder(app, ui, p, path, name, *photos, *videos, *bytes, ids),
        Focused::File(f) => file(app, ui, p, f),
    }
}

fn select_toggle(ui: &mut Ui, p: &Palette, tri: Tri, label_on: &str, label_off: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(150.0, 36.0), Sense::click());
    ui.painter().rect(rect, 10.0, if resp.hovered() { p.raised } else { p.surface }, egui::Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    let cb = Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), vec2(18.0, 18.0));
    theme::paint_check(ui.painter(), cb, tri, resp.hovered(), p);
    ui.painter().text(pos2(rect.left() + 38.0, rect.center().y), Align2::LEFT_CENTER, if tri == Tri::All { label_on } else { label_off }, theme::font(14.0, W::Medium), p.text);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

fn fact(ui: &mut Ui, p: &Palette, k: &str, v: String) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(96.0, 22.0), Sense::hover());
        ui.painter().text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, k, theme::font(14.0, W::Regular), p.muted);
        ui.add(egui::Label::new(txt(v, 14.0, W::Medium, p.text)).wrap());
    });
}

#[allow(clippy::too_many_arguments)]
fn folder(app: &mut App, ui: &mut Ui, p: &Palette, path: &str, name: &str, photos: usize, videos: usize, bytes: u64, ids: &[u64]) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, (w * 0.5).min(180.0)), Sense::hover());
    ui.painter().rect_filled(rect, 14.0, p.raised);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, ic::FOLDER, theme::icon_font(64.0), p.warn);
    ui.add_space(14.0);
    ui.add(egui::Label::new(txt(name, 18.0, W::Semibold, p.text)).wrap());
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if theme::ghost_button(ui, ic::FOLDER_OPEN, "Open").clicked() {
            app.results.navigate(path.to_string());
        }
        let tri = app.results.tri_of(ids);
        if select_toggle(ui, p, tri, "All selected", "Select all") {
            app.results.toggle_ids(ids);
        }
    });
    ui.add_space(14.0);
    fact(ui, p, "Photos", photos.to_string());
    fact(ui, p, "Videos", videos.to_string());
    fact(ui, p, "Total size", format_size(bytes));
    fact(ui, p, "Location", path.trim_start_matches('\\').replace('\\', " \u{203A} "));
}

fn file(app: &mut App, ui: &mut Ui, p: &Palette, f: &FoundFile) {
    let Some(dev) = app.session.as_ref().map(|s| s.dev.clone()) else { return };
    let w = ui.available_width();
    let h = (w * 0.75).min(340.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    ui.painter().rect_filled(rect, 14.0, p.raised);
    if f.category() == Category::Image && f.health != Health::Overwritten {
        app.previewer.request_large(&dev, f);
        match app.previewer.large.as_ref().map(|l| &l.1) {
            Some(Slot::Ready(tex, _)) => {
                let ts = tex.size_vec2();
                let s = (rect.width() / ts.x).min(rect.height() / ts.y);
                let r = Rect::from_center_size(rect.center(), ts * s);
                ui.painter().add(egui::epaint::RectShape::filled(r, 10, Color32::WHITE).with_texture(tex.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
            }
            Some(Slot::Failed(e)) => {
                ui.painter().text(rect.center() - vec2(0.0, 14.0), Align2::CENTER_CENTER, ic::IMAGE_BROKEN, theme::icon_font(34.0), p.faint);
                let msg = theme::ellipsize(ui, e, &theme::font(13.5, W::Regular), rect.width() - 30.0);
                ui.painter().text(rect.center() + vec2(0.0, 22.0), Align2::CENTER_CENTER, msg, theme::font(13.5, W::Regular), p.muted);
            }
            _ => {
                let t = ui.input(|i| i.time);
                let a = (0.3 + 0.25 * (t * 3.0).sin()) as f32;
                ui.painter().rect_filled(rect, 14.0, theme::lerp(p.raised, p.field, a));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
    } else if f.category() == Category::Video {
        ui.painter().text(rect.center() - vec2(0.0, 10.0), Align2::CENTER_CENTER, ic::FILM_STRIP, theme::icon_font(48.0), p.video);
        let s = f.info.summary();
        if !s.is_empty() {
            ui.painter().text(rect.center() + vec2(0.0, 34.0), Align2::CENTER_CENTER, s, theme::font(13.5, W::Medium), p.muted);
        }
    } else {
        ui.painter().text(rect.center() - vec2(0.0, 12.0), Align2::CENTER_CENTER, ic::PROHIBIT, theme::icon_font(38.0), p.bad);
        ui.painter().text(rect.center() + vec2(0.0, 24.0), Align2::CENTER_CENTER, "Data was overwritten", theme::font(13.5, W::Medium), p.muted);
    }
    ui.add_space(14.0);
    ui.add(egui::Label::new(txt(&f.name, 18.0, W::Semibold, p.text)).wrap());
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        theme::chip(ui, theme::state_text(f.state), p.muted);
        theme::chip(ui, theme::health_text(f.health), theme::health_color(p, f.health));
        if f.extents.len() > 1 {
            theme::chip(ui, &format!("{} pieces rejoined", f.extents.len()), p.accent);
        }
    });
    if !f.note.is_empty() {
        ui.add_space(2.0);
        ui.add(egui::Label::new(txt(&f.note, 14.0, W::Regular, theme::health_color(p, f.health))).wrap());
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let (icon, label) = if f.category() == Category::Video { (ic::PLAY, "Play") } else { (ic::ARROW_SQUARE_OUT, "Open") };
        if theme::ghost_button(ui, icon, label).on_hover_text("Opens a temporary copy saved on a different drive").clicked() {
            crate::recovery::open_external(app, f.clone());
        }
        let tri = if app.results.checked.contains(&f.id) { Tri::All } else { Tri::Off };
        if select_toggle(ui, p, tri, "Selected", "Select") {
            app.results.toggle_ids(&[f.id]);
        }
    });
    if let Some(msg) = app.recovery.opening_status() {
        ui.label(txt(msg, 13.5, W::Regular, p.muted));
    }
    ui.add_space(14.0);
    let loc = if f.origin == Origin::Carved {
        "Unknown, found by deep scan".to_string()
    } else {
        format!("{}{}", f.volume, f.path).replace('\\', " \u{203A} ").trim_end_matches(" \u{203A} ").to_string()
    };
    fact(ui, p, "Location", loc);
    fact(ui, p, "Size", format_size(f.size));
    fact(ui, p, "Type", f.format.name().to_string());
    if let Some(d) = f.info.dims() {
        fact(ui, p, "Dimensions", d);
    }
    if let Some(d) = f.info.duration {
        fact(ui, p, "Duration", format_duration(d));
    }
    if let Some(c) = &f.info.codec {
        fact(ui, p, "Codec", c.clone());
    }
    if let Some(c) = &f.info.camera {
        fact(ui, p, "Camera", c.clone());
    }
    if let Some(t) = f.info.taken {
        fact(ui, p, "Taken", format_date(t));
    }
    if let Some(t) = f.modified {
        fact(ui, p, "Modified", format_date(t));
    }
    fact(ui, p, "Found in", f.origin.label().to_string());
    ui.add_space(12.0);
    match app.previewer.verified.get(&f.id) {
        None => {
            if theme::ghost_button(ui, ic::SEAL_CHECK, "Check every byte").on_hover_text("Reads the whole file and validates its structure").clicked() {
                app.previewer.request_verify(&dev, f);
            }
        }
        Some(None) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(txt("Checking the whole file", 14.0, W::Regular, p.muted));
            });
        }
        Some(Some(v)) => {
            let (icon, t, c) = match v.health {
                Health::Excellent | Health::Good => (ic::SEAL_CHECK, "Structure fully intact".to_string(), p.good),
                Health::Damaged => (ic::WARNING, format!("Damaged. {}", v.note), p.warn),
                _ => (ic::X_CIRCLE, if v.note.is_empty() { "Not recoverable".into() } else { v.note.clone() }, p.bad),
            };
            ui.horizontal(|ui| {
                ui.label(theme::ico(icon, 18.0, c));
                ui.add(egui::Label::new(txt(t, 14.0, W::Medium, c)).wrap());
            });
        }
    }
}
