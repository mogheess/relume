//! Application state, scan session lifecycle, top bar and screen routing.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, vec2, Align, Layout, Sense};

use relume_core::formats::Category;
use relume_core::partition::VolumeRegion;
use relume_core::platform::{self, DriveInfo, DriveKind};
use relume_core::scan::{self, FolderFilter, ScanEvent, ScanRequest};
use relume_core::Dev;

use crate::preview::Previewer;
use crate::recovery::RecoveryUi;
use crate::results::Results;
use crate::scanview::SpaceMap;
use crate::theme::{self, ic, pal, txt, W};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    Quick,
    Full,
    Deep,
}

/// What the user chose to recover from.
#[derive(Clone)]
pub enum Source {
    Drive(DriveInfo),
    Folder { path: PathBuf, drive: DriveInfo, rel: String },
    RecycleBin(DriveInfo),
    Image(PathBuf),
}

impl Source {
    pub fn title(&self) -> String {
        match self {
            Source::Drive(d) => d.title(),
            Source::Folder { path, .. } => path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string()),
            Source::RecycleBin(_) => "Recycle Bin".into(),
            Source::Image(p) => p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        }
    }
    pub fn subtitle(&self) -> String {
        match self {
            Source::Drive(d) => relume_core::util::format_size(d.size),
            Source::Folder { path, .. } => path.display().to_string(),
            Source::RecycleBin(d) => format!("Drive {}:", d.letter.unwrap_or('C')),
            Source::Image(_) => "Disk image".into(),
        }
    }
    pub fn icon(&self) -> &'static str {
        match self {
            Source::Drive(d) => crate::home::drive_icon(d),
            Source::Folder { .. } => ic::FOLDER,
            Source::RecycleBin(_) => ic::TRASH,
            Source::Image(_) => ic::DISC,
        }
    }
    pub fn device_path(&self) -> String {
        match self {
            Source::Drive(d) | Source::Folder { drive: d, .. } | Source::RecycleBin(d) => d.path.clone(),
            Source::Image(p) => p.to_string_lossy().to_string(),
        }
    }
    pub fn folder_filter(&self) -> Option<FolderFilter> {
        match self {
            Source::Folder { rel, .. } => Some(FolderFilter::Path(rel.clone())),
            Source::RecycleBin(_) => Some(FolderFilter::RecycleBin),
            _ => None,
        }
    }
    /// What must not be written to while recovering from this source.
    pub fn protect(&self) -> Protect {
        match self {
            Source::Drive(d) if d.kind == DriveKind::Disk => Protect::Disk(d.disk_number),
            Source::Drive(d) | Source::Folder { drive: d, .. } | Source::RecycleBin(d) => Protect::Volume(d.volume_id.clone(), d.letter),
            // Writing next to an image file can't overwrite the image's contents.
            Source::Image(_) => Protect::Nothing,
        }
    }
}

pub enum Protect {
    Volume(Option<String>, Option<char>),
    Disk(Option<u32>),
    Nothing,
}

pub struct Session {
    pub source: Source,
    pub dev: Dev,
    rx: Receiver<ScanEvent>,
    pub cancel: Arc<AtomicBool>,
    pub phase: String,
    pub fraction: f32,
    pub scanned: u64,
    pub total: u64,
    pub started: Instant,
    pub finished: Option<(f64, bool, u32)>,
    pub volumes: Vec<VolumeRegion>,
    pub warnings: Vec<String>,
    rate: Option<(Instant, u64, f64)>,
}

impl Session {
    pub fn running(&self) -> bool {
        self.finished.is_none()
    }
    pub fn speed(&self) -> Option<f64> {
        self.rate.map(|r| r.2).filter(|s| *s > 0.0)
    }
    pub fn eta_secs(&self) -> Option<f64> {
        if self.fraction <= 0.03 || !self.running() {
            return None;
        }
        let el = self.started.elapsed().as_secs_f64();
        Some(el / self.fraction as f64 * (1.0 - self.fraction as f64))
    }
}

#[derive(PartialEq)]
pub enum Screen {
    Home,
    Results,
}

pub struct App {
    pub screen: Screen,
    pub drives: Vec<DriveInfo>,
    pub elevated: bool,
    pub selected: Option<Source>,
    pub want_images: bool,
    pub want_videos: bool,
    pub mode: ScanMode,
    pub free_only: bool,
    pub session: Option<Session>,
    pub results: Results,
    pub map: SpaceMap,
    pub scan_panel_open: bool,
    pub previewer: Previewer,
    pub recovery: RecoveryUi,
    pub error: Option<String>,
    pub confirm_leave: bool,
    pub logo: egui::TextureHandle,
    shot: Option<(PathBuf, u32)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> App {
        theme::setup(&cc.egui_ctx);
        let theme_env = if cfg!(feature = "dev-hooks") { std::env::var("RELUME_THEME").ok() } else { None };
        let theme_pref = match theme_env.as_deref() {
            Some("dark") => egui::ThemePreference::Dark,
            Some("system") => egui::ThemePreference::System,
            _ => egui::ThemePreference::Light,
        };
        cc.egui_ctx.set_theme(theme_pref);
        let mut app = App {
            screen: Screen::Home,
            drives: platform::list_drives(),
            elevated: platform::is_elevated(),
            selected: None,
            want_images: true,
            want_videos: true,
            mode: ScanMode::Full,
            free_only: true,
            session: None,
            results: Results::default(),
            map: SpaceMap::default(),
            scan_panel_open: true,
            previewer: Previewer::new(cc.egui_ctx.clone()),
            recovery: RecoveryUi::default(),
            error: None,
            confirm_leave: false,
            logo: theme::logo_texture(&cc.egui_ctx),
            shot: if cfg!(feature = "dev-hooks") { std::env::var("RELUME_SCREENSHOT").ok().map(|p| (PathBuf::from(p), 0)) } else { None },
        };
        // `Relume <disk image>` opens it straight away (also used for testing).
        if let Some(arg) = std::env::args().nth(1) {
            let p = PathBuf::from(arg);
            if p.exists() {
                app.selected = Some(Source::Image(p));
                if cfg!(feature = "dev-hooks") && std::env::var("RELUME_AUTOSCAN").is_ok() {
                    app.start_scan(&cc.egui_ctx);
                }
            }
        }
        app
    }

    pub fn refresh_drives(&mut self) {
        self.drives = platform::list_drives();
    }

    pub fn start_scan(&mut self, ctx: &egui::Context) {
        let Some(src) = self.selected.clone() else { return };
        let dev = match relume_core::open_path(&PathBuf::from(src.device_path())) {
            Ok(d) => d,
            Err(e) => {
                self.error = Some(if !self.elevated {
                    format!("Relume couldn't open {}.\n\nReading drives needs administrator rights.", src.title())
                } else {
                    format!("Relume couldn't open {}.\n\n{}", src.title(), e)
                });
                return;
            }
        };
        let mut cats = Vec::new();
        if self.want_images {
            cats.push(Category::Image);
        }
        if self.want_videos {
            cats.push(Category::Video);
        }
        let req = ScanRequest {
            quick: self.mode != ScanMode::Deep,
            deep: self.mode != ScanMode::Quick,
            categories: cats,
            folder: src.folder_filter(),
            deep_free_only: self.free_only,
            skip_inside_found: true,
        };
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (d2, c2, ctx2) = (dev.clone(), cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let (itx, irx) = mpsc::channel();
            let h = std::thread::spawn(move || scan::run_scan(d2, req, itx, c2));
            // Wake the UI at most ~12x per second; it drains everything queued each frame.
            let mut last = Instant::now() - std::time::Duration::from_secs(1);
            for ev in irx {
                let finished = matches!(ev, ScanEvent::Finished { .. });
                if tx.send(ev).is_err() {
                    break;
                }
                if finished || last.elapsed().as_millis() >= 80 {
                    ctx2.request_repaint();
                    last = Instant::now();
                }
            }
            let _ = h.join();
            ctx2.request_repaint();
        });
        self.results = Results::default();
        self.results.folder_scope = match &src {
            Source::Folder { rel, .. } => Some(rel.clone()),
            _ => None,
        };
        self.map = SpaceMap::new(dev.len());
        self.scan_panel_open = true;
        self.previewer.reset();
        self.session = Some(Session {
            source: src,
            dev,
            rx,
            cancel,
            phase: "Starting".into(),
            fraction: 0.0,
            scanned: 0,
            total: 0,
            started: Instant::now(),
            finished: None,
            volumes: Vec::new(),
            warnings: Vec::new(),
            rate: None,
        });
        self.screen = Screen::Results;
    }

    fn pump(&mut self) {
        let Some(s) = self.session.as_mut() else { return };
        let mut n = 0;
        while let Ok(ev) = s.rx.try_recv() {
            n += 1;
            match ev {
                ScanEvent::Phase(p) => s.phase = p,
                ScanEvent::Progress { fraction, scanned, total } => {
                    s.fraction = fraction;
                    s.total = total;
                    if scanned > 0 {
                        let now = Instant::now();
                        match s.rate {
                            Some((t, b, sp)) if now.duration_since(t).as_secs_f64() > 1.0 => {
                                let inst = scanned.saturating_sub(b) as f64 / now.duration_since(t).as_secs_f64();
                                s.rate = Some((now, scanned, if sp > 0.0 { sp * 0.6 + inst * 0.4 } else { inst }));
                            }
                            None => s.rate = Some((now, scanned, 0.0)),
                            _ => {}
                        }
                        s.scanned = scanned;
                    }
                }
                ScanEvent::Volumes(v) => s.volumes = v,
                ScanEvent::SpaceMap(m) => self.map.set_used(m),
                ScanEvent::Found(files) => {
                    self.map.add(&files);
                    self.results.add(files);
                }
                ScanEvent::Warning(w) => s.warnings.push(w),
                ScanEvent::Finished { secs, cancelled, bad_sectors } => {
                    s.finished = Some((secs, cancelled, bad_sectors));
                    s.fraction = 1.0;
                    s.scanned = s.total;
                    s.phase = if cancelled { "Scan stopped".into() } else { "Scan complete".into() };
                    self.scan_panel_open = false;
                }
            }
            if n > 4000 {
                break;
            }
        }
    }

    pub fn stop_scan(&mut self) {
        if let Some(s) = &self.session {
            s.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn go_home(&mut self) {
        self.stop_scan();
        self.session = None;
        self.results = Results::default();
        self.previewer.reset();
        self.screen = Screen::Home;
        self.refresh_drives();
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        egui::Panel::top("topbar")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(20, 12)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new((self.logo.id(), vec2(30.0, 30.0))));
                    ui.add_space(2.0);
                    ui.label(txt("Relume", 18.0, W::Semibold, p.text));
                    ui.add_space(18.0);
                    let step = match (&self.screen, self.session.as_ref().is_some_and(|s| s.running())) {
                        (Screen::Home, _) => 0,
                        (Screen::Results, true) => 1,
                        _ => 2,
                    };
                    if ui.available_width() > 640.0 {
                        for (i, label) in ["Choose location", "Scan", "Recover"].iter().enumerate() {
                            let (c, fill) = if i == step {
                                (p.text, p.raised)
                            } else if i < step {
                                (p.muted, egui::Color32::TRANSPARENT)
                            } else {
                                (p.faint, egui::Color32::TRANSPARENT)
                            };
                            let g = ui.painter().layout_no_wrap(format!("{}   {}", i + 1, label), theme::font(13.5, W::Medium), c);
                            let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 26.0, 32.0), Sense::hover());
                            ui.painter().rect_filled(r, 16.0, fill);
                            ui.painter().galley(egui::pos2(r.left() + 13.0, r.center().y - g.size().y / 2.0), g, c);
                            if i < 2 {
                                ui.label(theme::ico(ic::CARET_RIGHT, 13.0, p.faint));
                            }
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let dark = ui.visuals().dark_mode;
                        if theme::icon_button(ui, if dark { ic::SUN } else { ic::MOON }, if dark { "Light mode" } else { "Dark mode" }, false).clicked() {
                            ui.ctx().set_theme(if dark { egui::ThemePreference::Light } else { egui::ThemePreference::Dark });
                        }
                        if !self.elevated
                            && theme::ghost_button(ui, ic::SHIELD_CHECK, "Run as administrator").on_hover_text("Needed to read drives").clicked()
                            && platform::relaunch_elevated()
                        {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
            });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump();
        self.previewer.pump();
        self.top_bar(ui);
        match self.screen {
            Screen::Home => crate::home::show(self, ui),
            Screen::Results => crate::results::show(self, ui),
        }
        crate::recovery::show(self, &ctx);
        self.modals(&ctx);
        if self.session.as_ref().is_some_and(|s| s.running()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        }
        self.debug_screenshot(&ctx);
    }
}

impl App {
    fn modals(&mut self, ctx: &egui::Context) {
        if let Some(e) = self.error.clone() {
            egui::Modal::new(egui::Id::new("error")).show(ctx, |ui| {
                let p = pal(ui);
                ui.set_width(440.0);
                ui.horizontal(|ui| {
                    theme::icon_tile(ui, ic::WARNING, p.warn, 40.0);
                    ui.label(txt("Something went wrong", 18.0, W::Semibold, p.text));
                });
                ui.add_space(6.0);
                ui.label(txt(e, 14.0, W::Regular, p.muted));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if theme::primary_button(ui, "", "OK", true).clicked() {
                        self.error = None;
                    }
                    if !self.elevated && theme::ghost_button(ui, ic::SHIELD_CHECK, "Restart as administrator").clicked() && platform::relaunch_elevated() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        }
        if self.confirm_leave {
            egui::Modal::new(egui::Id::new("leave")).show(ctx, |ui| {
                let p = pal(ui);
                ui.set_width(420.0);
                ui.label(txt("Start a new scan?", 18.0, W::Semibold, p.text));
                ui.add_space(4.0);
                ui.label(txt("Files you haven't recovered yet will have to be found again.", 14.0, W::Regular, p.muted));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if theme::primary_button(ui, "", "Start over", true).clicked() {
                        self.confirm_leave = false;
                        self.go_home();
                    }
                    if theme::ghost_button(ui, "", "Keep results").clicked() {
                        self.confirm_leave = false;
                    }
                });
            });
        }
    }

    /// dev-hooks only. RELUME_SCREENSHOT=out.png: save a screenshot once the UI settles, then quit.
    fn debug_screenshot(&mut self, ctx: &egui::Context) {
        if !cfg!(feature = "dev-hooks") {
            return;
        }
        let Some((path, frames)) = self.shot.as_mut() else { return };
        *frames += 1;
        let frame = *frames;
        let path = path.clone();
        let view = std::env::var("RELUME_SHOT_VIEW").unwrap_or_default();
        if frame == 2 && view.contains("list") {
            self.results.view = crate::results::View::List;
        }
        if view.contains("images") && self.results.filter.cat.is_none() && self.session.as_ref().is_some_and(|s| !s.running()) {
            self.results.filter.cat = Some(Category::Image);
            self.results.dirty = true;
            self.results.select_first(view.contains("check"));
        }
        if view.contains("recover") && frame == 40 {
            let files = self.results.checked_files();
            crate::recovery::open_dialog(self, files);
        }
        let settle = std::env::var("RELUME_SCREENSHOT_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
        let done = view.contains("scanning") || self.session.as_ref().is_none_or(|s| !s.running());
        let frames = &mut self.shot.as_mut().unwrap().1;
        if *frames == settle && done {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            ctx.request_repaint();
        } else {
            if *frames >= settle && !done {
                *frames = settle - 1;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let px: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            let _ = image::save_buffer(path.as_path(), &px, w as u32, h as u32, image::ColorType::Rgba8);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
