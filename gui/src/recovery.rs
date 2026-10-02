//! Recovery dialog (destination, same-drive protection, progress, summary) and "open
//! externally" previews written to a safe temporary folder.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, vec2, Align, Layout, Sense};

use relume_core::model::{FoundFile, Health};
use relume_core::platform;
use relume_core::recover::{self, RecoverOptions, RecoverProgress, RecoverReport};
use relume_core::util::format_size;

use crate::app::{App, Protect};
use crate::theme::{self, ic, pal, txt, W};

#[derive(Default)]
pub enum Stage {
    #[default]
    Idle,
    Setup { files: Vec<FoundFile>, dest: Option<PathBuf>, include_overwritten: bool, same: bool },
    Running { progress: Arc<Mutex<RecoverProgress>>, cancel: Arc<AtomicBool>, report: Arc<Mutex<Option<RecoverReport>>> },
    Done(RecoverReport),
}

pub struct RecoveryUi {
    pub stage: Stage,
    pub keep_folders: bool,
    pub preview_dir: Option<PathBuf>,
    last_dest: Option<PathBuf>,
    opening: Option<(String, Arc<Mutex<Option<Result<PathBuf, String>>>>)>,
}

impl Default for RecoveryUi {
    fn default() -> Self {
        RecoveryUi { stage: Stage::Idle, keep_folders: true, preview_dir: None, last_dest: None, opening: None }
    }
}

impl RecoveryUi {
    pub fn opening_status(&mut self) -> Option<String> {
        let (name, slot) = self.opening.as_ref()?;
        let r = slot.lock().unwrap().clone();
        match r {
            None => Some(format!("Opening {}", name)),
            Some(Ok(_)) => {
                self.opening = None;
                None
            }
            Some(Err(e)) => Some(format!("Couldn't open it: {}", e)),
        }
    }
}

/// Would writing to `dest` land on the drive we're recovering from? Volume sources compare
/// volumes (C: to D: on the same SSD is fine); whole-disk sources compare physical disks.
fn same_drive(app: &App, dest: &std::path::Path) -> bool {
    let Some(s) = &app.session else { return false };
    match s.source.protect() {
        Protect::Volume(id, letter) => {
            let dst = platform::location_of(dest);
            match (id, dst.volume) {
                (Some(a), Some(b)) => a == b,
                _ => {
                    let first = dest.to_string_lossy().chars().next().map(|c| c.to_ascii_uppercase());
                    letter.is_some() && letter == first
                }
            }
        }
        Protect::Disk(n) => n.is_some() && platform::location_of(dest).disk == n,
        Protect::Nothing => false,
    }
}

pub fn open_dialog(app: &mut App, files: Vec<FoundFile>) {
    if files.is_empty() {
        return;
    }
    let dest = app.recovery.last_dest.clone();
    let same = dest.as_ref().is_some_and(|d| same_drive(app, d));
    app.recovery.stage = Stage::Setup { files, dest, include_overwritten: false, same };
}

fn start(app: &mut App, dest: PathBuf, files: Vec<FoundFile>) {
    let Some(s) = &app.session else { return };
    let dev = s.dev.clone();
    let progress = Arc::new(Mutex::new(RecoverProgress { files_total: files.len(), ..Default::default() }));
    let cancel = Arc::new(AtomicBool::new(false));
    let report = Arc::new(Mutex::new(None));
    app.recovery.last_dest = Some(dest.clone());
    let opts = RecoverOptions { dest, keep_folders: app.recovery.keep_folders };
    let (p2, c2, r2) = (progress.clone(), cancel.clone(), report.clone());
    std::thread::spawn(move || {
        let rep = recover::recover(&dev, &files, &opts, &c2, &mut |p| *p2.lock().unwrap() = p.clone());
        *r2.lock().unwrap() = Some(rep);
    });
    app.recovery.stage = Stage::Running { progress, cancel, report };
}

/// Export one file to a temp folder that is NOT on the source drive, then open it.
pub fn open_external(app: &mut App, f: FoundFile) {
    let Some(s) = &app.session else { return };
    let dev = s.dev.clone();
    let mut dir = app.recovery.preview_dir.clone().unwrap_or_else(|| std::env::temp_dir().join("Relume preview"));
    if same_drive(app, &dir) {
        let picked = rfd::FileDialog::new().set_title("Choose a folder on another drive for previews").pick_folder();
        match picked {
            Some(p) if !same_drive(app, &p) => {
                app.recovery.preview_dir = Some(p.clone());
                dir = p;
            }
            Some(_) => {
                app.error = Some("That folder is on the drive you're recovering from. Writing there could overwrite deleted files.".into());
                return;
            }
            None => return,
        }
    }
    let slot = Arc::new(Mutex::new(None));
    app.recovery.opening = Some((f.name.clone(), slot.clone()));
    std::thread::spawn(move || {
        let r = recover::write_into(&dev, &f, &dir, &AtomicBool::new(false), &mut |_| {})
            .map_err(|e| e.to_string())
            .and_then(|(p, _)| if platform::open_with_default(&p) { Ok(p) } else { Err("no app to open it".into()) });
        *slot.lock().unwrap() = Some(r);
    });
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let mut next: Option<Stage> = None;
    let mut go: Option<(PathBuf, Vec<FoundFile>)> = None;
    let mut pick = false;
    let keep = &mut app.recovery.keep_folders;
    match &mut app.recovery.stage {
        Stage::Idle => {}
        Stage::Setup { files, dest, include_overwritten, same } => {
            let total: u64 = files.iter().map(|f| f.size).sum();
            let over = files.iter().filter(|f| f.health == Health::Overwritten).count();
            let mut close = false;
            egui::Modal::new(egui::Id::new("recover")).show(ctx, |ui| {
                let p = pal(ui);
                ui.set_width(500.0);
                ui.horizontal(|ui| {
                    theme::icon_tile(ui, ic::ARROW_COUNTER_CLOCKWISE, p.accent, 44.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(txt(format!("Recover {} files", files.len()), 20.0, W::Semibold, p.text));
                        ui.label(txt(format_size(total), 14.0, W::Regular, p.muted));
                    });
                });
                ui.add_space(16.0);
                ui.label(txt("Save to", 14.0, W::Medium, p.muted));
                ui.add_space(2.0);
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
                ui.painter().rect_filled(r, 12.0, if resp.hovered() { p.raised } else { p.field });
                ui.painter().text(egui::pos2(r.left() + 22.0, r.center().y), egui::Align2::CENTER_CENTER, ic::FOLDER_OPEN, theme::icon_font(20.0), p.accent);
                let label = dest.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "Choose a folder on another drive".into());
                let shown = theme::ellipsize(ui, &label, &theme::font(14.0, W::Medium), r.width() - 140.0);
                ui.painter().text(egui::pos2(r.left() + 42.0, r.center().y), egui::Align2::LEFT_CENTER, shown, theme::font(14.0, W::Medium), if dest.is_some() { p.text } else { p.muted });
                ui.painter().text(egui::pos2(r.right() - 16.0, r.center().y), egui::Align2::RIGHT_CENTER, "Change", theme::font(14.0, W::Semibold), p.accent);
                if resp.clicked() {
                    pick = true;
                }
                if *same {
                    ui.add_space(8.0);
                    egui::Frame::new().fill(p.bad.gamma_multiply(0.12)).corner_radius(10).inner_margin(12).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(theme::ico(ic::WARNING, 18.0, p.bad));
                            ui.add(egui::Label::new(txt(
                                "This folder is on the drive you're recovering from. Saving here can overwrite files you haven't recovered yet. Pick another drive, like a USB stick.",
                                14.0,
                                W::Regular,
                                p.text,
                            )).wrap());
                        });
                    });
                }
                ui.add_space(12.0);
                theme::switch(ui, keep, "Recreate original folders");
                if over > 0 {
                    ui.add_space(4.0);
                    theme::switch(ui, include_overwritten, &format!("Include {} overwritten files", over));
                }
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        if theme::primary_button(ui, ic::ARROW_COUNTER_CLOCKWISE, if *same { "Recover anyway" } else { "Recover" }, dest.is_some()).clicked() {
                            if let Some(d) = dest.clone() {
                                let chosen: Vec<FoundFile> = files.iter().filter(|f| *include_overwritten || f.health != Health::Overwritten).cloned().collect();
                                go = Some((d, chosen));
                            }
                        }
                        if theme::ghost_button(ui, "", "Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            });
            if close {
                next = Some(Stage::Idle);
            }
        }
        Stage::Running { progress, cancel, report } => {
            if let Some(rep) = report.lock().unwrap().take() {
                next = Some(Stage::Done(rep));
            } else {
                let pr = progress.lock().unwrap().clone();
                egui::Modal::new(egui::Id::new("recovering")).show(ctx, |ui| {
                    let p = pal(ui);
                    ui.set_width(460.0);
                    ui.label(txt("Recovering", 20.0, W::Semibold, p.text));
                    ui.add_space(12.0);
                    let frac = if pr.bytes_total > 0 { pr.bytes_done as f32 / pr.bytes_total as f32 } else { 0.0 };
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
                    ui.painter().rect_filled(r, 5.0, p.field);
                    let mut fr = r;
                    fr.set_width((r.width() * frac).max(10.0));
                    ui.painter().rect_filled(fr, 5.0, p.accent);
                    ui.add_space(8.0);
                    ui.label(txt(format!("{} of {} files  ·  {} of {}", pr.files_done, pr.files_total, format_size(pr.bytes_done), format_size(pr.bytes_total)), 14.0, W::Medium, p.text));
                    ui.label(txt(theme::ellipsize(ui, &pr.current, &theme::font(13.5, W::Regular), 430.0), 13.5, W::Regular, p.muted));
                    ui.add_space(12.0);
                    if theme::ghost_button(ui, ic::STOP, "Stop").clicked() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        Stage::Done(rep) => {
            egui::Modal::new(egui::Id::new("done")).show(ctx, |ui| {
                let p = pal(ui);
                ui.set_width(480.0);
                let ok = rep.failed.is_empty() && !rep.cancelled;
                ui.horizontal(|ui| {
                    theme::icon_tile(ui, if ok { ic::CHECK_CIRCLE } else { ic::WARNING }, if ok { p.good } else { p.warn }, 44.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(txt(if ok { "Recovered" } else if rep.cancelled { "Recovery stopped" } else { "Recovered with some errors" }, 20.0, W::Semibold, p.text));
                        ui.label(txt(format!("{} files, {}", rep.recovered, format_size(rep.bytes)), 14.0, W::Regular, p.muted));
                    });
                });
                ui.add_space(8.0);
                ui.add(egui::Label::new(txt(rep.dest.display().to_string(), 14.0, W::Regular, p.muted)).wrap());
                if rep.bad_sectors > 0 {
                    ui.label(txt(format!("{} unreadable sectors were filled with zeros. The drive may be failing.", rep.bad_sectors), 14.0, W::Regular, p.warn));
                }
                for (n, e) in rep.failed.iter().take(5) {
                    ui.label(txt(format!("{}: {}", n, e), 13.5, W::Regular, p.bad));
                }
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        if theme::primary_button(ui, ic::FOLDER_OPEN, "Open folder", true).clicked() {
                            platform::open_with_default(&rep.dest);
                        }
                        if theme::ghost_button(ui, "", "Done").clicked() {
                            next = Some(Stage::Idle);
                        }
                    });
                });
            });
        }
    }
    if pick {
        if let Some(d) = rfd::FileDialog::new().set_title("Where should the recovered files go?").pick_folder() {
            let warn = same_drive(app, &d);
            if let Stage::Setup { dest, same, .. } = &mut app.recovery.stage {
                *dest = Some(d);
                *same = warn;
            }
        }
    }
    if let Some((d, f)) = go {
        start(app, d, f);
    } else if let Some(n) = next {
        app.recovery.stage = n;
    }
}
