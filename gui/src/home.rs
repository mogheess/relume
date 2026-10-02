//! Home: choose where to recover from (common folders, drives, whole disks, images) and how.

use std::path::{Path, PathBuf};

use eframe::egui::{self, vec2, Align, Color32, Layout, ScrollArea, Ui};

use relume_core::platform::{self, DriveInfo, DriveKind};
use relume_core::util::format_size;

use crate::app::{App, ScanMode, Source};
use crate::theme::{self, ic, pal, txt, Palette, W};

pub fn drive_icon(d: &DriveInfo) -> &'static str {
    match (d.kind == DriveKind::Disk, d.bus.as_str(), d.removable) {
        (_, "SD" | "MMC", _) => ic::SIM_CARD,
        (_, "USB", _) | (_, _, true) => ic::USB,
        (true, _, _) => ic::HARD_DRIVES,
        _ => ic::HARD_DRIVE,
    }
}

fn drive_color(p: &Palette, d: &DriveInfo) -> Color32 {
    match (d.bus.as_str(), d.removable) {
        ("SD" | "MMC", _) => p.video,
        ("USB", _) | (_, true) => p.photo,
        _ => p.accent,
    }
}

/// Volume for a path: (drive, path inside the volume like "\Users\me\Pictures").
fn drive_for_path(drives: &[DriveInfo], p: &Path) -> Option<(DriveInfo, String)> {
    let s = p.to_string_lossy();
    let letter = s.chars().next().filter(|c| c.is_ascii_alphabetic() && s.chars().nth(1) == Some(':'))?.to_ascii_uppercase();
    let loc = platform::location_of(p);
    let d = drives
        .iter()
        .find(|d| d.kind == DriveKind::Volume && loc.volume.is_some() && d.volume_id == loc.volume)
        .or_else(|| drives.iter().find(|d| d.kind == DriveKind::Volume && d.letter == Some(letter)))?;
    let rel = s.get(2..).unwrap_or("").to_string();
    Some((d.clone(), if rel.is_empty() { "\\".into() } else { rel }))
}

/// Columns and card width that exactly fill `avail`.
fn grid(avail: f32, min_w: f32, gap: f32, max_cols: usize) -> (usize, f32) {
    let cols = (((avail + gap) / (min_w + gap)).floor() as usize).clamp(1, max_cols);
    (cols, (avail - gap * (cols - 1) as f32) / cols as f32)
}

fn card_grid<T>(ui: &mut Ui, items: Vec<T>, min_w: f32, h: f32, max_cols: usize, mut draw: impl FnMut(&mut Ui, T, f32, f32)) {
    let gap = 14.0;
    let (cols, w) = grid(ui.available_width(), min_w, gap, max_cols);
    let mut it = items.into_iter().peekable();
    while it.peek().is_some() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for _ in 0..cols {
                match it.next() {
                    Some(x) => draw(ui, x, w, h),
                    None => break,
                }
            }
        });
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }
}

enum Place {
    Known(&'static str, &'static str, PathBuf, fn(&Palette) -> Color32),
    Bin(DriveInfo),
    Pick,
}

pub fn show(app: &mut App, ui: &mut Ui) {
    dock(app, ui);
    let p = pal(ui);
    egui::CentralPanel::no_frame().frame(egui::Frame::new().fill(p.bg)).show(ui, |ui| {
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let full = ui.available_width();
            let pad = if full > 1300.0 { (full - 1240.0) / 2.0 } else if full > 1000.0 { 40.0 } else { 24.0 };
            ui.add_space(20.0);
            ui.horizontal(|ui| {
                ui.add_space(pad);
                ui.vertical(|ui| {
                    ui.set_width(full - pad * 2.0);
                    content(app, ui, p);
                });
            });
            ui.add_space(32.0);
        });
    });
}

fn content(app: &mut App, ui: &mut Ui, p: &'static Palette) {
    ui.label(txt("Get your photos and videos back", 32.0, W::Semibold, p.text));
    ui.add_space(2.0);
    ui.label(txt("Choose where they were. Relume only reads, nothing on the drive is changed.", 16.0, W::Regular, p.muted));
    ui.add_space(30.0);

    theme::section_title(ui, "Quick locations");
    ui.add_space(6.0);
    let mut places: Vec<Place> = Vec::new();
    for (icon, name, path, color) in [
        (ic::DESKTOP, "Desktop", dirs::desktop_dir(), (|p: &Palette| p.accent) as fn(&Palette) -> Color32),
        (ic::IMAGES, "Pictures", dirs::picture_dir(), |p: &Palette| p.photo),
        (ic::FILM_STRIP, "Videos", dirs::video_dir(), |p: &Palette| p.video),
        (ic::DOWNLOAD_SIMPLE, "Downloads", dirs::download_dir(), |p: &Palette| p.good),
        (ic::FILE_TEXT, "Documents", dirs::document_dir(), |p: &Palette| p.warn),
    ] {
        if let Some(path) = path {
            if drive_for_path(&app.drives, &path).is_some() {
                places.push(Place::Known(icon, name, path, color));
            }
        }
    }
    if let Some(sys) = app.drives.iter().find(|d| d.system && d.kind == DriveKind::Volume) {
        places.push(Place::Bin(sys.clone()));
    }
    places.push(Place::Pick);
    let mut start = false;
    card_grid(ui, places, 190.0, 64.0, 7, |ui, place, w, h| {
        let (icon, name, color, sel, tip) = match &place {
            Place::Known(icon, name, path, color) => {
                (*icon, name.to_string(), color(p), matches!(&app.selected, Some(Source::Folder { path: x, .. }) if x == path), path.display().to_string())
            }
            Place::Bin(_) => (ic::TRASH, "Recycle Bin".into(), p.bad, matches!(app.selected, Some(Source::RecycleBin(_))), "Files deleted through the Recycle Bin, with their original names".into()),
            Place::Pick => {
                let custom = match &app.selected {
                    Some(Source::Folder { path, .. }) if !is_known(path) => Some(path.clone()),
                    _ => None,
                };
                let label = custom.as_ref().and_then(|c| c.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Browse to a folder".into());
                (ic::FOLDER_OPEN, label, p.muted, custom.is_some(), "Pick any folder".into())
            }
        };
        let r = theme::card(ui, sel, vec2(w, h), |ui, p| {
            theme::icon_tile(ui, icon, color, 38.0);
            ui.add_space(4.0);
            ui.label(txt(theme::ellipsize(ui, &name, &theme::font(15.0, W::Medium), w - 90.0), 15.0, W::Medium, p.text));
        });
        let r = r.on_hover_text(tip);
        if r.clicked() {
            match place {
                Place::Known(_, _, path, _) => {
                    if let Some((drive, rel)) = drive_for_path(&app.drives, &path) {
                        app.selected = Some(Source::Folder { path, drive, rel });
                    }
                }
                Place::Bin(d) => app.selected = Some(Source::RecycleBin(d)),
                Place::Pick => {
                    if let Some(path) = rfd::FileDialog::new().set_title("Folder the files were in").pick_folder() {
                        match drive_for_path(&app.drives, &path) {
                            Some((drive, rel)) => app.selected = Some(Source::Folder { path, drive, rel }),
                            None => app.error = Some("That folder isn't on a drive Relume can scan.".into()),
                        }
                    }
                }
            }
        }
        if r.double_clicked() {
            start = true;
        }
    });
    ui.add_space(26.0);

    theme::section_title(ui, "Drives and memory cards");
    ui.add_space(6.0);
    let vols: Vec<DriveInfo> = app.drives.iter().filter(|d| d.kind == DriveKind::Volume).cloned().collect();
    if vols.is_empty() {
        ui.label(txt("No drives found. Open a disk image below, or run Relume as administrator.", 14.0, W::Regular, p.muted));
    }
    card_grid(ui, vols, 300.0, 108.0, 4, |ui, d, w, h| {
        if drive_card(app, ui, &d, w, h) {
            start = true;
        }
    });
    ui.add_space(26.0);

    ui.horizontal(|ui| {
        theme::section_title(ui, "Whole disks and images");
        ui.label(txt("for formatted, RAW or lost partitions", 14.0, W::Regular, p.faint));
    });
    ui.add_space(6.0);
    let mut items: Vec<Option<DriveInfo>> = app.drives.iter().filter(|d| d.kind == DriveKind::Disk).cloned().map(Some).collect();
    items.push(None);
    card_grid(ui, items, 300.0, 108.0, 4, |ui, d, w, h| match d {
        Some(d) => {
            if drive_card(app, ui, &d, w, h) {
                start = true;
            }
        }
        None => {
            let sel_path = match &app.selected {
                Some(Source::Image(p)) => Some(p.clone()),
                _ => None,
            };
            let r = theme::card(ui, sel_path.is_some(), vec2(w, h), |ui, p| {
                theme::icon_tile(ui, ic::DISC, p.muted, 46.0);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.add_space(14.0);
                    let name = sel_path.as_ref().and_then(|x| x.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Open a disk image".into());
                    ui.label(txt(theme::ellipsize(ui, &name, &theme::font(15.5, W::Medium), w - 110.0), 15.5, W::Medium, p.text));
                    ui.label(txt(if sel_path.is_some() { "Disk image" } else { ".img  .dd  .raw  .bin" }, 13.5, W::Regular, p.muted));
                });
            });
            if r.clicked() {
                if let Some(path) = rfd::FileDialog::new().set_title("Open disk image").pick_file() {
                    app.selected = Some(Source::Image(path));
                }
            }
        }
    });
    if start {
        app.start_scan(ui.ctx());
    }
}

fn is_known(path: &Path) -> bool {
    [dirs::desktop_dir(), dirs::picture_dir(), dirs::video_dir(), dirs::download_dir(), dirs::document_dir()].iter().any(|d| d.as_deref() == Some(path))
}

/// Returns true on double-click (start scanning).
fn drive_card(app: &mut App, ui: &mut Ui, d: &DriveInfo, w: f32, h: f32) -> bool {
    let sel = matches!(&app.selected, Some(Source::Drive(x)) if x.path == d.path);
    let r = theme::card(ui, sel, vec2(w, h), |ui, p| {
        theme::icon_tile(ui, drive_icon(d), drive_color(p, d), 46.0);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(txt(theme::ellipsize(ui, &d.title(), &theme::font(15.5, W::Medium), w - 200.0), 15.5, W::Medium, p.text));
                let mut tags: Vec<String> = Vec::new();
                if !d.fs.is_empty() {
                    tags.push(d.fs.clone());
                }
                if !d.bus.is_empty() && w > 330.0 {
                    tags.push(d.bus.clone());
                }
                for t in tags.iter().take(2) {
                    theme::chip(ui, t, p.muted);
                }
            });
            let bar_w = (w - 100.0).max(80.0);
            match d.free {
                Some(free) if d.size > 0 => {
                    theme::usage_bar(ui, 1.0 - free as f32 / d.size as f32, bar_w, p);
                    ui.label(txt(format!("{} free of {}", format_size(free), format_size(d.size)), 13.5, W::Regular, p.muted));
                }
                _ => {
                    ui.label(txt(
                        if d.kind == DriveKind::Disk { format!("{}, all partitions", format_size(d.size)) } else { format_size(d.size) },
                        13.5,
                        W::Regular,
                        p.muted,
                    ));
                }
            }
        });
    });
    let r = r.on_hover_text(if d.kind == DriveKind::Disk { "Scans every partition and the space between them" } else { "Scans this drive" });
    if r.clicked() {
        app.selected = Some(Source::Drive(d.clone()));
    }
    r.double_clicked()
}

/// Bottom dock: selected location, what to look for, scan type, start.
fn dock(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    egui::Panel::bottom("dock")
        .frame(egui::Frame::new().fill(p.surface).stroke(egui::Stroke::new(1.0, p.border)).inner_margin(egui::Margin::symmetric(24, 14)))
        .show(ui, |ui| {
            let wide = ui.available_width() > 1120.0;
            let can = app.selected.is_some() && (app.want_images || app.want_videos);
            ui.horizontal(|ui| {
                source_summary(app, ui, p);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if theme::primary_button(ui, ic::MAGNIFYING_GLASS, "Start scan", can).clicked() {
                        app.start_scan(ui.ctx());
                    }
                    if wide {
                        ui.add_space(8.0);
                        options(app, ui);
                    }
                });
            });
            if !wide {
                ui.add_space(6.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    options(app, ui);
                });
            }
        });
}

fn source_summary(app: &App, ui: &mut Ui, p: &Palette) {
    match &app.selected {
        Some(s) => {
            theme::icon_tile(ui, s.icon(), p.accent, 40.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(txt(theme::ellipsize(ui, &s.title(), &theme::font(15.0, W::Medium), 280.0), 15.0, W::Medium, p.text));
                ui.label(txt(theme::ellipsize(ui, &s.subtitle(), &theme::font(13.5, W::Regular), 280.0), 13.5, W::Regular, p.muted));
            });
        }
        None => {
            theme::icon_tile(ui, ic::CURSOR_CLICK, p.faint, 40.0);
            ui.label(txt("Choose a location to scan", 15.0, W::Medium, p.muted));
        }
    }
}

fn options(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    // Laid out right to left inside the dock: scan type, then what to look for.
    let r = theme::icon_button(ui, ic::SLIDERS_HORIZONTAL, "More options", false);
    egui::Popup::menu(&r).show(|ui| {
        ui.set_min_width(300.0);
        ui.add_space(4.0);
        theme::switch(ui, &mut app.free_only, "Skip space used by existing files")
            .on_hover_text("Faster. Turn off if the drive was formatted or its file system is damaged.");
        ui.add_space(4.0);
    });
    theme::segmented(
        ui,
        "mode",
        &mut app.mode,
        &[(ScanMode::Quick, "", "Quick"), (ScanMode::Full, "", "Complete"), (ScanMode::Deep, "", "Deep only")],
        false,
    );
    ui.add_space(6.0);
    theme::toggle_chip(ui, &mut app.want_videos, ic::FILM_STRIP, "Videos", p.video);
    theme::toggle_chip(ui, &mut app.want_images, ic::IMAGE, "Photos", p.photo);
    ui.label(txt("Find", 14.0, W::Medium, p.muted));
}
