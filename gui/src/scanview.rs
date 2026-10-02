//! Live scan header: progress gauge, counters, and a map of the whole storage device that
//! fills in as Relume reads it (used space, searched space, where photos and videos were found).

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, Layout, Rect, Sense, Stroke, StrokeKind, Ui};

use relume_core::formats::Category;
use relume_core::model::{FileState, FoundFile};
use relume_core::util::{format_duration, format_size};

use crate::app::App;
use crate::theme::{self, ic, pal, txt, Palette, W};

const N: usize = 2048;

#[derive(Default)]
pub struct SpaceMap {
    total: u64,
    used: Vec<u8>,
    photos: Vec<u32>,
    videos: Vec<u32>,
}

impl SpaceMap {
    pub fn new(total: u64) -> SpaceMap {
        SpaceMap { total, used: Vec::new(), photos: vec![0; N], videos: vec![0; N] }
    }
    pub fn set_used(&mut self, v: Vec<u8>) {
        self.used = v;
    }
    pub fn add(&mut self, files: &[FoundFile]) {
        if self.total == 0 || self.photos.is_empty() {
            return;
        }
        for f in files.iter().filter(|f| f.state != FileState::Existing) {
            let Some(s) = f.start() else { continue };
            let b = ((s as u128 * N as u128) / self.total as u128).min(N as u128 - 1) as usize;
            if f.category() == Category::Video {
                self.videos[b] += 1;
            } else {
                self.photos[b] += 1;
            }
        }
    }
    fn cell(&self, a: usize, b: usize) -> (f32, u32, u32) {
        let b = b.max(a + 1).min(N);
        let used = if self.used.len() == N { self.used[a..b].iter().map(|&x| x as f32).sum::<f32>() / ((b - a) as f32 * 255.0) } else { 0.0 };
        let ph = self.photos.get(a..b).map(|s| s.iter().sum()).unwrap_or(0);
        let vi = self.videos.get(a..b).map(|s| s.iter().sum()).unwrap_or(0);
        (used, ph, vi)
    }
}

/// Draw the block map. `head` = fraction of the device already read (deep scan).
fn draw_map(ui: &mut Ui, map: &SpaceMap, head: Option<f32>, scanning: bool, rows: usize, p: &Palette) {
    let w = ui.available_width();
    let gap = 3.0;
    let target = 11.0;
    let cols = (((w + gap) / (target + gap)).floor() as usize).max(8);
    let cw = (w - gap * (cols - 1) as f32) / cols as f32;
    let ch = 11.0;
    let n = cols * rows;
    let (rect, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (ch + gap) - gap), Sense::hover());
    let painter = ui.painter_at(rect.expand(6.0));
    let t = ui.input(|i| i.time);
    let head_cell = head.map(|h| ((h * n as f32) as usize).min(n - 1));
    let scanned_tint = p.map_read;
    for i in 0..n {
        let (a, b) = (i * N / n, (i + 1) * N / n);
        let (used, ph, vi) = map.cell(a, b);
        let x = rect.left() + (i % cols) as f32 * (cw + gap);
        let y = rect.top() + (i / cols) as f32 * (ch + gap);
        let r = Rect::from_min_size(pos2(x, y), vec2(cw, ch));
        let read = head.is_some_and(|h| (i + 1) as f32 / n as f32 <= h) || (!scanning && head.is_some());
        let col = if ph + vi > 0 {
            if ph >= vi { p.photo } else { p.video }
        } else if used > 0.55 {
            p.map_used
        } else if read {
            scanned_tint
        } else {
            p.map_free
        };
        painter.rect_filled(r, 2.5, col);
        if scanning && head_cell == Some(i) {
            let pulse = (0.55 + 0.45 * (t * 5.0).sin()) as f32;
            painter.rect_filled(r.expand(3.0), 4.0, p.accent.gamma_multiply(0.25 * pulse));
            painter.rect_filled(r, 2.5, theme::lerp(p.accent, Color32::WHITE, 0.25 * pulse));
        }
    }
}

/// Stacked bar of the device layout: partitions and unpartitioned space.
fn draw_layout_bar(ui: &mut Ui, app: &App, p: &Palette) {
    let Some(s) = &app.session else { return };
    let total = s.dev.len().max(1);
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    ui.painter().rect_filled(rect, 8.0, p.map_free);
    let colors = [p.accent, p.photo, p.good, p.video, p.warn];
    for (i, v) in s.volumes.iter().enumerate() {
        let x0 = rect.left() + rect.width() * (v.offset as f32 / total as f32);
        let x1 = rect.left() + rect.width() * ((v.offset + v.size) as f32 / total as f32);
        let r = Rect::from_min_max(pos2(x0, rect.top()), pos2(x1.max(x0 + 3.0).min(rect.right()), rect.bottom()));
        let c = colors[i % colors.len()];
        ui.painter().rect_filled(r, 8.0, c.gamma_multiply(if v.lost { 0.22 } else { 0.32 }));
        ui.painter().rect_stroke(r, 8.0, Stroke::new(1.0, c.gamma_multiply(0.6)), StrokeKind::Inside);
        let fs = v.fs.map(|f| f.label()).unwrap_or("Unknown");
        let label = if v.lost {
            format!("Lost {} partition  {}", fs, format_size(v.size))
        } else if v.name.is_empty() {
            format!("{}  {}", fs, format_size(v.size))
        } else {
            format!("{}  {}  {}", v.name, fs, format_size(v.size))
        };
        let f = theme::font(12.5, W::Medium);
        let fitted = theme::ellipsize(ui, &label, &f, r.width() - 16.0);
        if r.width() > 40.0 && fitted.chars().count() > 3 {
            ui.painter().text(pos2(r.left() + 9.0, r.center().y), Align2::LEFT_CENTER, fitted, f, p.text);
        }
    }
}

fn legend(ui: &mut Ui, p: &Palette) {
    let scanned = p.map_read;
    for (c, label) in [(p.photo, "Photos"), (p.video, "Videos"), (scanned, "Searched"), (p.map_used, "In use"), (p.map_free, "Not yet searched")] {
        let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
        ui.painter().rect_filled(r, 3.0, c);
        ui.label(txt(label, 13.0, W::Regular, p.muted));
        ui.add_space(8.0);
    }
}

fn stat(ui: &mut Ui, p: &Palette, value: String, label: &str, color: Color32, w: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
    ui.painter().rect_filled(rect, 12.0, p.raised);
    ui.painter().circle_filled(pos2(rect.left() + 16.0, rect.top() + 20.0), 4.0, color);
    ui.painter().text(pos2(rect.left() + 28.0, rect.top() + 20.0), Align2::LEFT_CENTER, label, theme::font(13.0, W::Medium), p.muted);
    ui.painter().text(pos2(rect.left() + 14.0, rect.top() + 42.0), Align2::LEFT_CENTER, value, theme::font(18.0, W::Semibold), p.text);
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    let Some(s) = &app.session else { return };
    let running = s.running();
    let photos = app.results.found_count(Category::Image);
    let videos = app.results.found_count(Category::Video);
    let open = app.scan_panel_open;
    let mut stop = false;
    let mut back = false;
    let mut toggle = false;
    egui::Panel::top("scanhead")
        .frame(egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).inner_margin(egui::Margin::symmetric(24, 16)))
        .show(ui, |ui| {
            let w = ui.available_width();
            ui.horizontal(|ui| {
                // Gauge
                let gsize = if open { 76.0 } else { 44.0 };
                let (gr, _) = ui.allocate_exact_size(vec2(gsize, gsize), Sense::hover());
                let sweep = (running && s.fraction < 0.02).then(|| ui.input(|i| i.time) * 0.6 % 1.0);
                theme::ring_gauge(ui.painter(), gr.center(), gsize / 2.0, s.fraction, p, sweep);
                if running {
                    ui.painter().text(gr.center(), Align2::CENTER_CENTER, format!("{:.0}%", s.fraction * 100.0), theme::font(if open { 17.0 } else { 12.5 }, W::Semibold), p.text);
                } else {
                    ui.painter().text(gr.center(), Align2::CENTER_CENTER, ic::CHECK, theme::icon_font(if open { 26.0 } else { 18.0 }), p.good);
                }
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    if open {
                        ui.add_space(10.0);
                    }
                    let title = if running { format!("Scanning {}", s.source.title()) } else { s.source.title() };
                    ui.label(txt(theme::ellipsize(ui, &title, &theme::font(19.0, W::Semibold), (w * 0.32).max(200.0)), 19.0, W::Semibold, p.text));
                    let sub = match s.finished {
                        Some((secs, cancelled, bad)) => {
                            let mut t = format!("{} in {}", if cancelled { "Stopped" } else { "Finished" }, format_duration(secs));
                            t.push_str(&format!(", {} photos and {} videos found", photos, videos));
                            if bad > 0 {
                                t.push_str(&format!(", {} unreadable sectors skipped", bad));
                            }
                            t
                        }
                        None => s.phase.clone(),
                    };
                    ui.label(txt(sub, 14.0, W::Regular, p.muted));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if theme::icon_button(ui, if open { ic::CARET_UP } else { ic::CARET_DOWN }, if open { "Hide storage map" } else { "Show storage map" }, false).clicked() {
                        toggle = true;
                    }
                    if running {
                        if theme::ghost_button(ui, ic::STOP, "Stop").clicked() {
                            stop = true;
                        }
                    } else if theme::ghost_button(ui, ic::ARROW_COUNTER_CLOCKWISE, "New scan").clicked() {
                        back = true;
                    }
                    if open {
                        let cw = 136.0;
                        let room = ui.available_width() - 20.0;
                        let k = ((room + 10.0) / (cw + 10.0)).floor() as usize;
                        let mut stats: Vec<(String, &str, Color32)> = vec![(photos.to_string(), "Photos", p.photo), (videos.to_string(), "Videos", p.video)];
                        if s.total > 0 {
                            stats.push((format!("{} / {}", format_size(s.scanned), format_size(s.total)), "Searched", p.accent));
                        }
                        if running {
                            if let Some(eta) = s.eta_secs() {
                                stats.push((format_duration(eta), "Time left", p.muted));
                            } else if let Some(sp) = s.speed() {
                                stats.push((format!("{}/s", format_size(sp as u64)), "Speed", p.muted));
                            }
                        }
                        for (v, l, c) in stats.into_iter().take(k).rev() {
                            let width = if l == "Searched" { cw + 40.0 } else { cw };
                            stat(ui, p, v, l, c, width);
                        }
                    }
                });
            });
            if open {
                ui.add_space(14.0);
                if !s.volumes.is_empty() {
                    draw_layout_bar(ui, app, p);
                    ui.add_space(8.0);
                }
                let head = (s.total > 0 && s.scanned > 0).then(|| s.scanned as f32 / s.total as f32);
                let rows = if ui.ctx().content_rect().height() > 900.0 { 7 } else { 5 };
                draw_map(ui, &app.map, head, running, rows, p);
                ui.add_space(8.0);
                ui.horizontal(|ui| legend(ui, p));
                for w in s.warnings.iter().take(2) {
                    ui.horizontal(|ui| {
                        ui.label(theme::ico(ic::WARNING, 16.0, p.warn));
                        ui.label(txt(w, 13.5, W::Regular, p.warn));
                    });
                }
                let deleted = app.results.set.files.iter().filter(|f| f.state == FileState::Deleted).count();
                let zeroed = app.results.set.files.iter().filter(|f| f.state == FileState::Deleted && f.note.contains("zeroed")).count();
                if deleted >= 5 && zeroed * 2 > deleted {
                    ui.horizontal(|ui| {
                        ui.label(theme::ico(ic::WARNING, 16.0, p.warn));
                        ui.label(txt("Most deleted files read back as zeros. SSDs erase deleted data right away (TRIM), so those can't be recovered by any tool.", 13.5, W::Regular, p.warn));
                    });
                }
            }
        });
    if toggle {
        app.scan_panel_open = !app.scan_panel_open;
    }
    if stop {
        app.stop_scan();
    }
    if back {
        if app.results.set.is_empty() {
            app.go_home();
        } else {
            app.confirm_leave = true;
        }
    }
}
