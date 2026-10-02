//! Results, browsed like File Explorer: original folders with breadcrumbs, back and up, plus
//! an "All files" view. Sidebar filters, grid and list layouts, selection, action bar.

use std::collections::{BTreeMap, HashMap, HashSet};

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, Key, Layout, Modifiers, Rect, ScrollArea, Sense, Stroke, StrokeKind, Ui};
use egui_extras::{Column, TableBuilder};

use relume_core::formats::{Category, Format};
use relume_core::model::{FileState, FoundFile, Health, Origin, ResultSet};
use relume_core::util::{format_date, format_duration, format_size};

use crate::app::App;
use crate::preview::Slot;
use crate::theme::{self, ic, pal, txt, Palette, Tri, W};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Grid,
    List,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Folders,
    AllFiles,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Date,
    Name,
    Size,
    Type,
    Health,
}

impl SortKey {
    fn label(self) -> &'static str {
        match self {
            SortKey::Date => "Date",
            SortKey::Name => "Name",
            SortKey::Size => "Size",
            SortKey::Type => "Type",
            SortKey::Health => "Condition",
        }
    }
}

pub struct Filter {
    pub cat: Option<Category>,
    pub fmt: Option<Format>,
    pub state: Option<FileState>,
    pub show_existing: bool,
    pub hide_overwritten: bool,
    pub hide_small: bool,
    pub search: String,
}

impl Default for Filter {
    fn default() -> Self {
        Filter { cat: None, fmt: None, state: None, show_existing: false, hide_overwritten: false, hide_small: true, search: String::new() }
    }
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    /// Every file below this folder.
    ids: Vec<u64>,
    /// Files directly in this folder.
    files: Vec<u64>,
    photos: usize,
    videos: usize,
    bytes: u64,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Item {
    Dir(String),
    File(u64),
}

/// What the details panel shows.
pub enum Focused {
    None,
    File(FoundFile),
    Dir { path: String, name: String, photos: usize, videos: usize, bytes: u64, ids: Vec<u64> },
}

pub struct Results {
    pub set: ResultSet,
    pub folder_scope: Option<String>,
    pub checked: HashSet<u64>,
    pub focus: Option<Item>,
    anchor: Option<usize>,
    pub filter: Filter,
    pub view: View,
    pub mode: Mode,
    pub sort: (SortKey, bool),
    pub tile: f32,
    pub cwd: String,
    back: Vec<String>,
    items: Vec<Item>,
    pub dirty: bool,
    by_id: HashMap<u64, usize>,
    g_cat: HashMap<Category, Vec<u64>>,
    g_fmt: BTreeMap<Format, Vec<u64>>,
    g_state: BTreeMap<FileState, Vec<u64>>,
    tree: Node,
    open_dirs: HashSet<String>,
    pub sidebar: Option<bool>,
    pub inspector: bool,
    scroll_to: Option<usize>,
    cols: usize,
}

impl Default for Results {
    fn default() -> Self {
        Results {
            set: ResultSet::new(),
            folder_scope: None,
            checked: HashSet::new(),
            focus: None,
            anchor: None,
            filter: Filter::default(),
            view: View::Grid,
            mode: Mode::Folders,
            sort: (SortKey::Date, false),
            tile: 190.0,
            cwd: String::new(),
            back: Vec::new(),
            items: Vec::new(),
            dirty: true,
            by_id: HashMap::new(),
            g_cat: HashMap::new(),
            g_fmt: BTreeMap::new(),
            g_state: BTreeMap::new(),
            tree: Node::default(),
            open_dirs: HashSet::new(),
            sidebar: None,
            inspector: true,
            scroll_to: None,
            cols: 1,
        }
    }
}

const SMALL: u64 = 10 * 1024;
const DEEP: &str = "Found by deep scan";

fn parts(path: &str) -> impl Iterator<Item = &str> {
    path.split('\\').filter(|p| !p.is_empty())
}

impl Results {
    pub fn add(&mut self, files: Vec<FoundFile>) {
        for f in files {
            if let Some(i) = self.set.add(f) {
                let id = self.set.files[i].id;
                self.by_id.insert(id, i);
            }
        }
        self.dirty = true;
    }

    pub fn get(&self, id: u64) -> Option<&FoundFile> {
        self.by_id.get(&id).map(|&i| &self.set.files[i])
    }

    pub fn found_count(&self, c: Category) -> usize {
        self.set.files.iter().filter(|f| f.state != FileState::Existing && f.category() == c).count()
    }

    fn base_ok(&self, f: &FoundFile) -> bool {
        let fl = &self.filter;
        (fl.show_existing || f.state != FileState::Existing)
            && (!fl.hide_small || f.size >= SMALL || f.category() == Category::Video)
            && (!fl.hide_overwritten || f.health != Health::Overwritten)
            && (fl.search.is_empty() || f.name.to_lowercase().contains(&fl.search.to_lowercase()))
    }

    /// Folder a file is shown under: "\Volume\original\path", or the deep-scan type folder.
    fn folder_of(f: &FoundFile) -> String {
        if f.origin == Origin::Carved {
            format!("\\{}\\{}", DEEP, f.format.name())
        } else {
            let vol = if f.volume.trim().is_empty() { "Drive".to_string() } else { f.volume.trim().to_string() };
            format!("\\{}{}", vol, f.path.trim_end_matches('\\'))
        }
    }

    fn node(&self, path: &str) -> Option<&Node> {
        let mut n = &self.tree;
        for p in parts(path) {
            n = n.children.get(p)?;
        }
        Some(n)
    }

    fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.g_cat.clear();
        self.g_fmt.clear();
        self.g_state.clear();
        self.tree = Node::default();
        for f in &self.set.files {
            if !self.base_ok(f) {
                continue;
            }
            self.g_state.entry(f.state).or_default().push(f.id);
            if self.filter.state.is_some_and(|s| s != f.state) {
                continue;
            }
            self.g_cat.entry(f.category()).or_default().push(f.id);
            self.g_fmt.entry(f.format).or_default().push(f.id);
            if self.filter.cat.is_some_and(|c| c != f.category()) || self.filter.fmt.is_some_and(|x| x != f.format) {
                continue;
            }
            let video = f.category() == Category::Video;
            let add = |n: &mut Node| {
                n.ids.push(f.id);
                n.bytes += f.size;
                if video {
                    n.videos += 1
                } else {
                    n.photos += 1
                }
            };
            let mut n = &mut self.tree;
            add(n);
            for part in parts(&Self::folder_of(f)) {
                n = n.children.entry(part.to_string()).or_default();
                add(n);
            }
            n.files.push(f.id);
        }
        if self.open_dirs.is_empty() {
            for k in self.tree.children.keys() {
                self.open_dirs.insert(format!("\\{}", k));
            }
        }
        // A single drive: open it directly, like Explorer opening a drive.
        if self.cwd.is_empty() && self.back.is_empty() && self.mode == Mode::Folders {
            let drives: Vec<&String> = self.tree.children.keys().filter(|k| k.as_str() != DEEP).collect();
            if drives.len() == 1 {
                self.cwd = format!("\\{}", drives[0]);
            }
        }
        self.rebuild_items();
    }

    fn rebuild_items(&mut self) {
        let Some(node) = self.node(&self.cwd) else {
            self.items.clear();
            return;
        };
        let mut files: Vec<u64> = match self.mode {
            Mode::Folders => node.files.clone(),
            Mode::AllFiles => node.ids.clone(),
        };
        let (key, asc) = self.sort;
        let get = |id: &u64| &self.set.files[self.by_id[id]];
        files.sort_by(|a, b| {
            let (x, y) = (get(a), get(b));
            let o = match key {
                SortKey::Name => x.name.to_lowercase().cmp(&y.name.to_lowercase()),
                SortKey::Size => x.size.cmp(&y.size),
                SortKey::Date => x.date().cmp(&y.date()),
                SortKey::Type => x.format.cmp(&y.format),
                SortKey::Health => x.health.cmp(&y.health),
            };
            let o = if asc { o } else { o.reverse() };
            o.then(x.id.cmp(&y.id))
        });
        let mut items: Vec<Item> = Vec::new();
        if self.mode == Mode::Folders {
            let mut dirs: Vec<&String> = node.children.keys().collect();
            dirs.sort_by_key(|d| (d.as_str() == DEEP, d.to_lowercase()));
            items.extend(dirs.into_iter().map(|d| Item::Dir(format!("{}\\{}", self.cwd, d))));
        }
        items.extend(files.into_iter().map(Item::File));
        self.items = items;
    }

    pub fn navigate(&mut self, path: String) {
        if path != self.cwd {
            self.back.push(std::mem::replace(&mut self.cwd, path));
            self.focus = None;
            self.anchor = None;
            self.scroll_to = Some(0);
            self.rebuild_items();
        }
    }

    fn go_back(&mut self) {
        if let Some(p) = self.back.pop() {
            self.cwd = p;
            self.focus = None;
            self.rebuild_items();
        }
    }

    fn go_up(&mut self) {
        if let Some((parent, _)) = self.cwd.rsplit_once('\\') {
            let parent = parent.to_string();
            self.navigate(parent);
        }
    }

    fn ids_of(&self, item: &Item) -> Vec<u64> {
        match item {
            Item::File(id) => vec![*id],
            Item::Dir(p) => self.node(p).map(|n| n.ids.clone()).unwrap_or_default(),
        }
    }

    fn tri(&self, ids: &[u64]) -> Tri {
        let n = ids.iter().filter(|id| self.checked.contains(id)).count();
        if n == 0 {
            Tri::Off
        } else if n == ids.len() {
            Tri::All
        } else {
            Tri::Some
        }
    }

    fn toggle_group(&mut self, ids: &[u64]) {
        if self.tri(ids) == Tri::All {
            for id in ids {
                self.checked.remove(id);
            }
        } else {
            self.checked.extend(ids.iter().copied());
        }
    }

    /// Plain click focuses; Ctrl/Cmd or the checkbox toggles; Shift ticks a range.
    fn click(&mut self, idx: usize, mods: Modifiers, on_checkbox: bool) {
        let item = self.items[idx].clone();
        if on_checkbox || mods.command {
            let ids = self.ids_of(&item);
            self.toggle_group(&ids);
            self.anchor = Some(idx);
        } else if mods.shift {
            let a = self.anchor.unwrap_or(idx);
            for k in a.min(idx)..=a.max(idx) {
                let ids = self.ids_of(&self.items[k].clone());
                self.checked.extend(ids);
            }
        } else {
            self.anchor = Some(idx);
        }
        self.focus = Some(item);
    }

    /// Double-click / Enter: open folders, tick files.
    fn activate(&mut self, idx: usize) {
        match self.items[idx].clone() {
            Item::Dir(p) => self.navigate(p),
            Item::File(id) => {
                if !self.checked.remove(&id) {
                    self.checked.insert(id);
                }
            }
        }
    }

    pub fn select_first(&mut self, check: bool) {
        self.refresh();
        let first = self.items.iter().position(|i| matches!(i, Item::File(_)));
        self.focus = first.map(|k| self.items[k].clone());
        if check {
            let ids: Vec<u64> = self.items.iter().filter_map(|i| if let Item::File(id) = i { Some(*id) } else { None }).take(3).collect();
            self.checked.extend(ids);
        }
    }

    pub fn checked_files(&self) -> Vec<FoundFile> {
        let mut v: Vec<FoundFile> = self.checked.iter().filter_map(|id| self.get(*id).cloned()).collect();
        v.sort_by_key(|f| f.id);
        v
    }

    pub fn focused(&self) -> Focused {
        match &self.focus {
            Some(Item::File(id)) => self.get(*id).cloned().map(Focused::File).unwrap_or(Focused::None),
            Some(Item::Dir(p)) => match self.node(p) {
                Some(n) => Focused::Dir {
                    path: p.clone(),
                    name: parts(p).last().unwrap_or("").to_string(),
                    photos: n.photos,
                    videos: n.videos,
                    bytes: n.bytes,
                    ids: n.ids.clone(),
                },
                None => Focused::None,
            },
            None => Focused::None,
        }
    }

    pub fn toggle_ids(&mut self, ids: &[u64]) {
        self.toggle_group(ids);
    }

    pub fn tri_of(&self, ids: &[u64]) -> Tri {
        self.tri(ids)
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    app.results.refresh();
    crate::scanview::show(app, ui);
    action_bar(app, ui);
    let width = ui.available_width();
    let sidebar = app.results.sidebar.unwrap_or(width >= 1080.0);
    let p = pal(ui);
    if sidebar {
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(272.0)
            .min_size(220.0)
            .max_size(380.0)
            .frame(egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).inner_margin(egui::Margin { left: 12, right: 10, top: 14, bottom: 14 }))
            .show(ui, |ui| {
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| sidebar_ui(app, ui));
            });
    }
    // Details column: always visible on wide windows; on smaller ones only while something
    // is selected, and narrower. It never floats over the toolbar or files.
    let wide = width >= 1320.0;
    let focused = app.results.focused();
    let has_focus = !matches!(focused, Focused::None);
    let show_details = app.results.inspector && (wide || has_focus);
    if show_details {
        let (def, min) = if wide { (360.0, 300.0) } else { (300.0, 260.0) };
        egui::Panel::right(if wide { "inspector" } else { "inspector_narrow" })
            .resizable(true)
            .default_size(def)
            .min_size(min)
            .max_size(520.0)
            .frame(egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).inner_margin(egui::Margin::same(18)))
            .show(ui, |ui| {
                if !wide {
                    ui.horizontal(|ui| {
                        ui.label(txt("Details", 15.0, W::Semibold, p.text));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if theme::icon_button(ui, ic::X, "Close", false).clicked() {
                                app.results.focus = None;
                            }
                        });
                    });
                }
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| crate::inspector::show(app, ui, &focused));
            });
    }
    egui::CentralPanel::no_frame().frame(egui::Frame::new().fill(p.bg)).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin { left: 20, right: 20, top: 14, bottom: 0 }).show(ui, |ui| {
            toolbar(app, ui, sidebar, show_details);
            ui.add_space(12.0);
            keyboard(app, ui);
            if app.results.items.is_empty() {
                empty_state(app, ui);
            } else {
                match app.results.view {
                    View::Grid => grid(app, ui),
                    View::List => list(app, ui),
                }
            }
        });
    });
}

fn empty_state(app: &App, ui: &mut Ui) {
    let p = pal(ui);
    let running = app.session.as_ref().is_some_and(|s| s.running());
    ui.add_space(80.0);
    ui.vertical_centered(|ui| {
        ui.label(theme::ico(if running { ic::MAGNIFYING_GLASS } else { ic::FOLDER_DASHED }, 44.0, p.faint));
        ui.add_space(6.0);
        ui.label(txt(if running { "Looking for photos and videos" } else { "Nothing here" }, 18.0, W::Semibold, p.text));
        if !running {
            ui.label(txt("Try another folder, or change the filters.", 14.0, W::Regular, p.muted));
        }
    });
}

// ---------------------------------------------------------------- sidebar

struct RowOut {
    clicked: bool,
    check: bool,
    caret: bool,
}

#[allow(clippy::too_many_arguments)]
fn side_row(ui: &mut Ui, p: &Palette, depth: usize, caret: Option<bool>, tri: Option<Tri>, icon: &str, color: Color32, label: &str, count: usize, active: bool) -> RowOut {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
    let hovered = resp.hovered();
    if active {
        ui.painter().rect_filled(rect, 8.0, p.accent_soft);
    } else if hovered {
        ui.painter().rect_filled(rect, 8.0, p.raised);
    }
    let mut x = rect.left() + 4.0 + depth as f32 * 16.0;
    let mut out = RowOut { clicked: false, check: false, caret: false };
    if let Some(open) = caret {
        let r = Rect::from_center_size(pos2(x + 9.0, rect.center().y), vec2(20.0, 26.0));
        let cr = ui.interact(r, resp.id.with("caret"), Sense::click());
        ui.painter().text(r.center(), Align2::CENTER_CENTER, if open { ic::CARET_DOWN } else { ic::CARET_RIGHT }, theme::icon_font(13.0), if cr.hovered() { p.text } else { p.faint });
        out.caret = cr.clicked();
    }
    x += 20.0;
    if let Some(t) = tri {
        let r = Rect::from_center_size(pos2(x + 9.0, rect.center().y), vec2(18.0, 18.0));
        let cb = ui.interact(r.expand(3.0), resp.id.with("check"), Sense::click());
        theme::paint_check(ui.painter(), r, t, cb.hovered(), p);
        out.check = cb.clicked();
        x += 28.0;
    }
    ui.painter().text(pos2(x + 9.0, rect.center().y), Align2::CENTER_CENTER, icon, theme::icon_font(17.0), color);
    x += 26.0;
    let cnt = count.to_string();
    let cf = theme::font(13.0, W::Medium);
    let cw = ui.painter().layout_no_wrap(cnt.clone(), cf.clone(), p.muted).size().x;
    let lf = theme::font(14.0, if active { W::Semibold } else { W::Regular });
    let label = theme::ellipsize(ui, label, &lf, rect.right() - x - cw - 20.0);
    ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, lf, if active { p.accent } else { p.text });
    ui.painter().text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, cnt, cf, p.faint);
    out.clicked = resp.clicked() && !out.check && !out.caret;
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    out
}

fn side_header(ui: &mut Ui, p: &Palette, t: &str) {
    ui.add_space(8.0);
    ui.label(txt(t, 13.0, W::Semibold, p.muted));
    ui.add_space(2.0);
}

fn sidebar_ui(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    let r = &mut app.results;
    ui.spacing_mut().item_spacing.y = 1.0;
    side_header(ui, p, "Locations");
    let tree = std::mem::take(&mut r.tree);
    let mut nav = None;
    let mut toggle_ids: Option<Vec<u64>> = None;
    let mut flip: Option<String> = None;
    folder_rows(ui, p, r, &tree, String::new(), 0, &mut nav, &mut toggle_ids, &mut flip);
    r.tree = tree;
    if let Some(n) = nav {
        r.navigate(n);
    }
    if let Some(ids) = toggle_ids {
        r.toggle_group(&ids);
    }
    if let Some(k) = flip {
        if !r.open_dirs.remove(&k) {
            r.open_dirs.insert(k);
        }
    }
    if let Some(scope) = &r.folder_scope {
        ui.add_space(4.0);
        ui.label(txt(format!("Scan limited to {}", scope), 13.5, W::Regular, p.muted));
    }

    ui.add_space(14.0);
    side_header(ui, p, "File types");
    let all: usize = r.g_cat.values().map(|v| v.len()).sum();
    let o = side_row(ui, p, 0, None, None, ic::SQUARES_FOUR, p.muted, "Everything", all, r.filter.cat.is_none() && r.filter.fmt.is_none());
    if o.clicked {
        r.filter.cat = None;
        r.filter.fmt = None;
        r.dirty = true;
    }
    for (cat, icon, color) in [(Category::Image, ic::IMAGE, p.photo), (Category::Video, ic::FILM_STRIP, p.video)] {
        let n = r.g_cat.get(&cat).map(|v| v.len()).unwrap_or(0);
        if n == 0 {
            continue;
        }
        let key = format!("cat{:?}", cat);
        let open = r.open_dirs.contains(&key);
        let label = if cat == Category::Image { "Photos" } else { "Videos" };
        let o = side_row(ui, p, 0, Some(open), None, icon, color, label, n, r.filter.cat == Some(cat) && r.filter.fmt.is_none());
        if o.caret && !r.open_dirs.remove(&key) {
            r.open_dirs.insert(key.clone());
        }
        if o.clicked {
            r.filter.cat = if r.filter.cat == Some(cat) && r.filter.fmt.is_none() { None } else { Some(cat) };
            r.filter.fmt = None;
            r.dirty = true;
        }
        if open {
            let fmts: Vec<(Format, usize)> = r.g_fmt.iter().filter(|(f, _)| f.category() == cat).map(|(f, v)| (*f, v.len())).collect();
            for (f, n) in fmts {
                let o = side_row(ui, p, 1, None, None, ic::FILE, p.faint, f.name(), n, r.filter.fmt == Some(f));
                if o.clicked {
                    r.filter.fmt = if r.filter.fmt == Some(f) { None } else { Some(f) };
                    r.filter.cat = None;
                    r.dirty = true;
                }
            }
        }
    }
    ui.add_space(14.0);
    side_header(ui, p, "Status");
    let states: Vec<(FileState, usize)> = r.g_state.iter().map(|(s, v)| (*s, v.len())).collect();
    for (st, n) in states {
        let (icon, color, label) = match st {
            FileState::Deleted => (ic::TRASH, p.muted, "Deleted"),
            FileState::RecycleBin => (ic::RECYCLE, p.good, "Recycle Bin"),
            FileState::Lost => (ic::MAGNIFYING_GLASS, p.photo, "Found by deep scan"),
            FileState::Existing => (ic::FILE, p.faint, "Not deleted"),
        };
        let o = side_row(ui, p, 0, None, None, icon, color, label, n, r.filter.state == Some(st));
        if o.clicked {
            r.filter.state = if r.filter.state == Some(st) { None } else { Some(st) };
            r.dirty = true;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn folder_rows(ui: &mut Ui, p: &Palette, r: &Results, node: &Node, prefix: String, depth: usize, nav: &mut Option<String>, toggle: &mut Option<Vec<u64>>, flip: &mut Option<String>) {
    let mut names: Vec<&String> = node.children.keys().collect();
    names.sort_by_key(|d| (d.as_str() == DEEP, d.to_lowercase()));
    for name in names {
        let child = &node.children[name];
        let path = format!("{}\\{}", prefix, name);
        let has_kids = !child.children.is_empty();
        let open = r.open_dirs.contains(&path);
        let icon = if depth == 0 {
            if name == DEEP { ic::MAGNIFYING_GLASS } else { ic::HARD_DRIVE }
        } else if name == "Lost Folders" {
            ic::FOLDER_DASHED
        } else if open && has_kids {
            ic::FOLDER_OPEN
        } else {
            ic::FOLDER
        };
        let color = if depth == 0 { p.accent } else { p.warn };
        let o = side_row(ui, p, depth, has_kids.then_some(open), Some(r.tri(&child.ids)), icon, color, name, child.ids.len(), r.cwd == path);
        if o.clicked {
            *nav = Some(path.clone());
        }
        if o.check {
            *toggle = Some(child.ids.clone());
        }
        if o.caret {
            *flip = Some(path.clone());
        }
        if has_kids && open {
            folder_rows(ui, p, r, child, path, depth + 1, nav, toggle, flip);
        }
    }
}

// ---------------------------------------------------------------- toolbar

fn toolbar(app: &mut App, ui: &mut Ui, sidebar: bool, details: bool) {
    let p = pal(ui);
    let r = &mut app.results;
    // Row 1: navigation, breadcrumbs, search
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if theme::icon_button(ui, ic::SIDEBAR_SIMPLE, if sidebar { "Hide sidebar" } else { "Show sidebar" }, sidebar).clicked() {
            r.sidebar = Some(!sidebar);
        }
        let can_back = !r.back.is_empty();
        let can_up = !r.cwd.is_empty();
        if nav_button(ui, ic::ARROW_LEFT, "Back", can_back).clicked() {
            r.go_back();
        }
        if nav_button(ui, ic::ARROW_UP, "Up one folder", can_up).clicked() {
            r.go_up();
        }
        let search_w = (ui.available_width() * 0.32).clamp(170.0, 300.0);
        let crumbs_w = ui.available_width() - search_w - 10.0;
        let (cr, _) = ui.allocate_exact_size(vec2(crumbs_w, 38.0), Sense::hover());
        ui.painter().rect(cr, 10.0, p.surface, Stroke::new(1.0, p.border), StrokeKind::Inside);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(cr.shrink2(vec2(8.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
        if let Some(path) = breadcrumbs(&mut child, p, &r.cwd, crumbs_w - 20.0) {
            r.navigate(path);
        }
        search_box(ui, p, &mut r.filter.search, search_w, &mut r.dirty);
    });
    ui.add_space(8.0);
    // Row 2: mode, count, filter, sort, view
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let narrow = ui.available_width() < 640.0;
        if theme::segmented(ui, "mode", &mut r.mode, &[(Mode::Folders, ic::FOLDER, "Folders"), (Mode::AllFiles, ic::SQUARES_FOUR, "All files")], narrow) {
            r.rebuild_items();
        }
        let files = r.items.iter().filter(|i| matches!(i, Item::File(_))).count();
        let dirs = r.items.len() - files;
        if !narrow {
            let plural = |n: usize, w: &str| if n == 1 { format!("1 {}", w) } else { format!("{} {}s", n, w) };
            let mut t = Vec::new();
            if dirs > 0 {
                t.push(plural(dirs, "folder"));
            }
            if files > 0 || dirs == 0 {
                t.push(plural(files, "file"));
            }
            ui.add_space(4.0);
            ui.label(txt(t.join(", "), 14.0, W::Regular, p.muted));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if theme::icon_button(ui, ic::INFO, if details { "Hide details" } else { "Show details" }, details).clicked() {
                r.inspector = !details;
            }
            theme::segmented(ui, "view", &mut r.view, &[(View::Grid, ic::SQUARES_FOUR, ""), (View::List, ic::LIST_BULLETS, "")], true);
            let sr = menu_button(ui, ic::SORT_DESCENDING, if narrow { "" } else { r.sort.0.label() }, false);
            egui::Popup::menu(&sr).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.set_min_width(230.0);
                for k in [SortKey::Date, SortKey::Name, SortKey::Size, SortKey::Type, SortKey::Health] {
                    if menu_item(ui, k.label(), r.sort.0 == k) {
                        r.sort.0 = k;
                        r.rebuild_items();
                    }
                }
                ui.separator();
                for (asc, label) in [(false, "Newest or largest first"), (true, "Oldest or smallest first")] {
                    if menu_item(ui, label, r.sort.1 == asc) {
                        r.sort.1 = asc;
                        r.rebuild_items();
                    }
                }
            });
            let active = r.filter.hide_overwritten || r.filter.show_existing || !r.filter.hide_small;
            let fr = menu_button(ui, ic::FUNNEL, if narrow { "" } else { "Filter" }, active);
            egui::Popup::menu(&fr).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.set_min_width(300.0);
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.add_space(4.0);
                let mut changed = false;
                changed |= theme::switch(ui, &mut r.filter.hide_small, "Hide tiny images").on_hover_text("Icons and web graphics under 10 KB").changed();
                changed |= theme::switch(ui, &mut r.filter.hide_overwritten, "Only recoverable files").changed();
                changed |= theme::switch(ui, &mut r.filter.show_existing, "Include files that weren't deleted").changed();
                if changed {
                    r.dirty = true;
                }
                ui.add_space(4.0);
            });
            if r.view == View::Grid && ui.available_width() > 300.0 {
                ui.add_sized(vec2(100.0, 18.0), egui::Slider::new(&mut r.tile, 140.0..=300.0).show_value(false)).on_hover_text("Thumbnail size");
                ui.label(theme::ico(ic::IMAGE, 16.0, p.muted));
            }
        });
    });
}

fn nav_button(ui: &mut Ui, icon: &str, tip: &str, enabled: bool) -> egui::Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 36.0), if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, 10.0, p.raised);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, theme::icon_font(18.0), if enabled { p.text } else { p.faint });
    resp.on_hover_text(tip)
}

/// Clickable path segments; long paths collapse from the left. Returns a path to open.
fn breadcrumbs(ui: &mut Ui, p: &Palette, cwd: &str, max_w: f32) -> Option<String> {
    let mut segs: Vec<(String, String)> = vec![("All locations".into(), String::new())];
    let mut acc = String::new();
    for part in parts(cwd) {
        acc = format!("{}\\{}", acc, part);
        segs.push((part.to_string(), acc.clone()));
    }
    let f = theme::font(14.0, W::Medium);
    let width = |s: &[(String, String)]| -> f32 { s.iter().map(|(n, _)| ui.painter().layout_no_wrap(n.clone(), f.clone(), p.text).size().x + 30.0).sum() };
    let mut start = 0;
    while start + 1 < segs.len() && width(&segs[start..]) > max_w {
        start += 1;
    }
    let mut out = None;
    ui.spacing_mut().item_spacing.x = 2.0;
    if start > 0 {
        ui.label(txt("…", 14.0, W::Medium, p.faint));
        ui.label(theme::ico(ic::CARET_RIGHT, 12.0, p.faint));
    }
    let last = segs.len() - 1;
    for (i, (name, path)) in segs.iter().enumerate().skip(start) {
        let is_last = i == last;
        let g = ui.painter().layout_no_wrap(name.clone(), f.clone(), p.text);
        let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 30.0), if is_last { Sense::hover() } else { Sense::click() });
        if !is_last && resp.hovered() {
            ui.painter().rect_filled(r, 7.0, p.raised);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.painter().galley(pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0), g, if is_last { p.text } else { p.muted });
        if resp.clicked() {
            out = Some(path.clone());
        }
        if !is_last {
            ui.label(theme::ico(ic::CARET_RIGHT, 12.0, p.faint));
        }
    }
    out
}

fn search_box(ui: &mut Ui, p: &Palette, text: &mut String, w: f32, dirty: &mut bool) {
    let (sr, _) = ui.allocate_exact_size(vec2(w, 38.0), Sense::hover());
    ui.painter().rect(sr, 10.0, p.surface, Stroke::new(1.0, p.border), StrokeKind::Inside);
    ui.painter().text(pos2(sr.left() + 18.0, sr.center().y), Align2::CENTER_CENTER, ic::MAGNIFYING_GLASS, theme::icon_font(16.0), p.muted);
    let edit = egui::TextEdit::singleline(text)
        .hint_text(txt("Search by name", 14.0, W::Regular, p.faint))
        .frame(egui::Frame::NONE)
        .font(theme::font(14.0, W::Regular))
        .desired_width(w - 46.0);
    let inner = Rect::from_min_max(pos2(sr.left() + 34.0, sr.top() + 9.0), pos2(sr.right() - 10.0, sr.bottom() - 7.0));
    if ui.put(inner, edit).changed() {
        *dirty = true;
    }
}

fn menu_button(ui: &mut Ui, icon: &str, label: &str, active: bool) -> egui::Response {
    let p = pal(ui);
    let f = theme::font(13.5, W::Medium);
    let (rect, resp) = ui.allocate_exact_size(vec2(theme::icon_label_width(ui.painter(), icon, label, &f) + 28.0, 38.0), Sense::click());
    let fill = if active {
        p.accent_soft
    } else if resp.hovered() {
        p.raised
    } else {
        p.surface
    };
    ui.painter().rect(rect, 10.0, fill, Stroke::new(1.0, if active { p.accent } else { p.border }), StrokeKind::Inside);
    theme::paint_icon_label(ui.painter(), rect.center(), icon, label, &f, if active { p.accent } else { p.text });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn menu_item(ui: &mut Ui, label: &str, selected: bool) -> bool {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 8.0, p.raised);
    }
    if selected {
        ui.painter().text(pos2(rect.left() + 14.0, rect.center().y), Align2::CENTER_CENTER, ic::CHECK, theme::icon_font(15.0), p.accent);
    }
    ui.painter().text(pos2(rect.left() + 30.0, rect.center().y), Align2::LEFT_CENTER, label, theme::font(14.0, W::Regular), if selected { p.text } else { p.muted });
    resp.clicked()
}

// ---------------------------------------------------------------- keyboard

fn keyboard(app: &mut App, ui: &mut Ui) {
    if ui.ctx().memory(|m| m.focused().is_some()) {
        return;
    }
    let r = &mut app.results;
    let step = if r.view == View::Grid { r.cols.max(1) as i64 } else { 1 };
    let (mv, space, all, esc, enter, up, back) = ui.input(|i| {
        let alt = i.modifiers.alt;
        let mut mv = 0i64;
        if i.key_pressed(Key::ArrowRight) && !alt {
            mv = 1;
        }
        if i.key_pressed(Key::ArrowLeft) && !alt {
            mv = -1;
        }
        if i.key_pressed(Key::ArrowDown) {
            mv = step;
        }
        if i.key_pressed(Key::ArrowUp) && !alt {
            mv = -step;
        }
        (
            mv,
            i.key_pressed(Key::Space),
            i.modifiers.command && i.key_pressed(Key::A),
            i.key_pressed(Key::Escape),
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::Backspace) || (alt && i.key_pressed(Key::ArrowUp)),
            alt && i.key_pressed(Key::ArrowLeft),
        )
    });
    if up {
        r.go_up();
        return;
    }
    if back {
        r.go_back();
        return;
    }
    if r.items.is_empty() {
        return;
    }
    let pos = r.focus.as_ref().and_then(|f| r.items.iter().position(|i| i == f));
    if all {
        let ids: Vec<u64> = r.items.iter().flat_map(|i| r.ids_of(i)).collect();
        r.checked.extend(ids);
    }
    if esc {
        r.focus = None;
    }
    if enter {
        if let Some(k) = pos {
            r.activate(k);
            return;
        }
    }
    if space {
        if let Some(it) = r.focus.clone() {
            let ids = r.ids_of(&it);
            r.toggle_group(&ids);
        }
    }
    if mv != 0 {
        let next = (pos.map(|p| p as i64).unwrap_or(-1) + mv).clamp(0, r.items.len() as i64 - 1) as usize;
        r.focus = Some(r.items[next].clone());
        r.anchor = Some(next);
        r.scroll_to = Some(next);
    }
}

// ---------------------------------------------------------------- grid

fn grid(app: &mut App, ui: &mut Ui) {
    let Some(dev) = app.session.as_ref().map(|s| s.dev.clone()) else { return };
    let p = pal(ui);
    let gap = 14.0;
    let avail = ui.available_width();
    let cols = (((avail + gap) / (app.results.tile + gap)).floor() as usize).max(1);
    let tw = (avail - gap * (cols - 1) as f32) / cols as f32;
    let img_h = (tw * 0.72).round();
    let th = img_h + 58.0;
    app.results.cols = cols;
    let n = app.results.items.len();
    let rows = n.div_ceil(cols);
    let mut clicks: Vec<(usize, Modifiers, bool, bool)> = Vec::new();
    let mut sa = ScrollArea::vertical().auto_shrink([false, false]);
    if let Some(i) = app.results.scroll_to.take() {
        sa = sa.vertical_scroll_offset(((i / cols) as f32 * (th + gap)).max(0.0));
    }
    let mods = ui.input(|i| i.modifiers);
    sa.show_rows(ui, th + gap, rows, |ui, range| {
        for row in range {
            let (row_rect, _) = ui.allocate_exact_size(vec2(avail, th + gap), Sense::hover());
            for c in 0..cols {
                let k = row * cols + c;
                if k >= n {
                    break;
                }
                let item = app.results.items[k].clone();
                let rect = Rect::from_min_size(pos2(row_rect.left() + c as f32 * (tw + gap), row_rect.top()), vec2(tw, th));
                let resp = ui.interact(rect, ui.id().with(("tile", &item)), Sense::click());
                let focused = app.results.focus.as_ref() == Some(&item);
                let hovered = resp.hovered();
                let ids = app.results.ids_of(&item);
                let tri = app.results.tri(&ids);
                let border = if hovered && !focused { p.faint.gamma_multiply(0.7) } else { p.border };
                ui.painter().rect(rect, 14.0, if focused { p.accent_soft } else { p.surface }, Stroke::new(1.0, border), StrokeKind::Inside);
                let img = Rect::from_min_size(rect.min + vec2(7.0, 7.0), vec2(tw - 14.0, img_h - 7.0));
                let (title, meta) = match &item {
                    Item::Dir(path) => {
                        let (ph, vi) = app.results.node(path).map(|n| (n.photos, n.videos)).unwrap_or((0, 0));
                        ui.painter().rect_filled(img, 10.0, p.raised);
                        let is_top = parts(path).count() == 1;
                        let (icon, col) = if is_top {
                            (if path.ends_with(DEEP) { ic::MAGNIFYING_GLASS } else { ic::HARD_DRIVE }, p.accent)
                        } else {
                            (ic::FOLDER, p.warn)
                        };
                        ui.painter().text(img.center(), Align2::CENTER_CENTER, icon, theme::icon_font((img.height() * 0.5).min(88.0)), col);
                        let mut m = Vec::new();
                        if ph > 0 {
                            m.push(format!("{} photo{}", ph, if ph == 1 { "" } else { "s" }));
                        }
                        if vi > 0 {
                            m.push(format!("{} video{}", vi, if vi == 1 { "" } else { "s" }));
                        }
                        (parts(path).last().unwrap_or("").to_string(), m.join(", "))
                    }
                    Item::File(id) => {
                        let f = app.results.get(*id).cloned().unwrap();
                        file_thumb(app, ui, &dev, &f, img, p);
                        (f.name.clone(), format!("{}  ·  {}", format_size(f.size), f.format.name()))
                    }
                };
                // Checkbox: shown when something inside is ticked, or on hover.
                let cb = Rect::from_min_size(img.min + vec2(8.0, 8.0), vec2(22.0, 22.0));
                let cbr = ui.interact(cb.expand(4.0), ui.id().with(("cb", &item)), Sense::click());
                if tri != Tri::Off || hovered || cbr.hovered() {
                    if tri == Tri::Off {
                        ui.painter().rect_filled(cb, 7.0, Color32::from_white_alpha(235));
                    }
                    theme::paint_check(ui.painter(), cb, tri, cbr.hovered(), p);
                }
                let font = theme::font(13.5, W::Medium);
                ui.painter().text(pos2(rect.left() + 12.0, rect.top() + img_h + 17.0), Align2::LEFT_CENTER, theme::ellipsize(ui, &title, &font, tw - 24.0), font, p.text);
                let mf = theme::font(13.0, W::Regular);
                ui.painter().text(pos2(rect.left() + 12.0, rect.top() + img_h + 38.0), Align2::LEFT_CENTER, theme::ellipsize(ui, &meta, &mf, tw - 24.0), mf, p.muted);
                if tri == Tri::All {
                    ui.painter().rect_stroke(rect, 14.0, Stroke::new(2.0, p.accent), StrokeKind::Inside);
                }
                let resp = match &item {
                    Item::File(id) => {
                        let f = app.results.get(*id).unwrap();
                        resp.on_hover_text(format!("{}\n{}{}", f.full_path(), theme::health_text(f.health), if f.note.is_empty() { String::new() } else { format!(". {}", f.note) }))
                    }
                    Item::Dir(_) => resp.on_hover_text("Double-click to open"),
                };
                if cbr.clicked() {
                    clicks.push((k, mods, true, false));
                } else if resp.double_clicked() {
                    clicks.push((k, mods, false, true));
                } else if resp.clicked() {
                    clicks.push((k, mods, false, false));
                }
                if hovered {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        }
    });
    for (k, m, cb, dbl) in clicks {
        if dbl {
            app.results.activate(k);
            break;
        }
        app.results.click(k, m, cb);
    }
}

fn file_thumb(app: &mut App, ui: &mut Ui, dev: &relume_core::Dev, f: &FoundFile, img: Rect, p: &Palette) {
    ui.painter().rect_filled(img, 10.0, p.raised);
    if f.category() == Category::Image && f.health != Health::Overwritten {
        match app.previewer.thumb(dev, f) {
            Slot::Ready(tex, [w, h]) => {
                // Cover-fit: crop the texture to the tile's aspect ratio.
                let (iw, ih) = (*w as f32, *h as f32);
                let (ta, ia) = (img.width() / img.height(), iw / ih);
                let uv = if ia > ta {
                    let s = ta / ia;
                    Rect::from_min_max(pos2((1.0 - s) / 2.0, 0.0), pos2((1.0 + s) / 2.0, 1.0))
                } else {
                    let s = ia / ta;
                    Rect::from_min_max(pos2(0.0, (1.0 - s) / 2.0), pos2(1.0, (1.0 + s) / 2.0))
                };
                ui.painter().add(egui::epaint::RectShape::filled(img, 10, Color32::WHITE).with_texture(tex.id(), uv));
            }
            Slot::Loading => {
                let t = ui.input(|i| i.time);
                let a = (0.4 + 0.3 * (t * 3.0).sin()) as f32;
                ui.painter().rect_filled(img, 10.0, theme::lerp(p.raised, p.field, a));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            }
            Slot::Failed(_) => {
                ui.painter().text(img.center(), Align2::CENTER_CENTER, ic::IMAGE_BROKEN, theme::icon_font(30.0), p.faint);
            }
        }
    } else if f.category() == Category::Video {
        ui.painter().text(img.center(), Align2::CENTER_CENTER, ic::FILM_STRIP, theme::icon_font((img.height() * 0.32).min(46.0)), p.video);
        if let Some(d) = f.info.dims() {
            ui.painter().text(pos2(img.left() + 10.0, img.bottom() - 16.0), Align2::LEFT_CENTER, d, theme::font(12.5, W::Medium), p.muted);
        }
        if let Some(d) = f.info.duration {
            let g = ui.painter().layout_no_wrap(format_duration(d), theme::font(12.5, W::Semibold), Color32::WHITE);
            let r = Rect::from_min_size(pos2(img.right() - g.size().x - 20.0, img.bottom() - 28.0), vec2(g.size().x + 12.0, 20.0));
            ui.painter().rect_filled(r, 6.0, Color32::from_black_alpha(170));
            ui.painter().galley(pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0), g, Color32::WHITE);
        }
    } else {
        ui.painter().text(img.center(), Align2::CENTER_CENTER, ic::PROHIBIT, theme::icon_font(30.0), p.bad);
    }
    // Condition badge only when there's a problem (keeps the grid calm).
    if matches!(f.health, Health::Damaged | Health::Overwritten) {
        let c = theme::health_color(p, f.health);
        let g = ui.painter().layout_no_wrap(theme::health_text(f.health).to_string(), theme::font(12.0, W::Semibold), Color32::WHITE);
        let br = Rect::from_min_size(pos2(img.right() - g.size().x - 20.0, img.top() + 8.0), vec2(g.size().x + 12.0, 20.0));
        ui.painter().rect_filled(br, 6.0, c);
        ui.painter().galley(pos2(br.left() + 6.0, br.center().y - g.size().y / 2.0), g, Color32::WHITE);
    }
}

// ---------------------------------------------------------------- list

fn list(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    let r = &mut app.results;
    r.cols = 1;
    let mods = ui.input(|i| i.modifiers);
    let mut clicks: Vec<(usize, bool, bool)> = Vec::new();
    let mut tb = TableBuilder::new(ui)
        .striped(false)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::exact(30.0))
        .column(Column::remainder().at_least(180.0).clip(true))
        .column(Column::initial(90.0))
        .column(Column::initial(136.0))
        .column(Column::initial(110.0))
        .column(Column::initial(120.0))
        .auto_shrink([false, false])
        .min_scrolled_height(0.0);
    if let Some(row) = r.scroll_to.take() {
        tb = tb.scroll_to_row(row, None);
    }
    tb.header(32.0, |mut h| {
        for t in ["", "Name", "Size", "Date", "Type", "Condition"] {
            h.col(|ui| {
                ui.label(txt(t, 13.0, W::Semibold, p.muted));
            });
        }
    })
    .body(|body| {
        body.rows(42.0, r.items.len(), |mut row| {
            let k = row.index();
            let item = r.items[k].clone();
            row.set_selected(r.focus.as_ref() == Some(&item));
            let ids = r.ids_of(&item);
            let tri = r.tri(&ids);
            row.col(|ui| {
                if theme::tri_check(ui, tri, 18.0).clicked() {
                    clicks.push((k, true, false));
                }
            });
            match &item {
                Item::Dir(path) => {
                    let (ph, vi, bytes) = r.node(path).map(|n| (n.photos, n.videos, n.bytes)).unwrap_or((0, 0, 0));
                    row.col(|ui| {
                        theme::icon_tile(ui, ic::FOLDER, p.warn, 28.0);
                        ui.label(txt(parts(path).last().unwrap_or(""), 14.0, W::Medium, p.text));
                    });
                    row.col(|ui| {
                        ui.label(txt(format_size(bytes), 13.5, W::Regular, p.muted));
                    });
                    row.col(|ui| {
                        ui.label(txt(format!("{} items", ph + vi), 13.5, W::Regular, p.muted));
                    });
                    row.col(|ui| {
                        ui.label(txt("Folder", 13.5, W::Regular, p.muted));
                    });
                    row.col(|_| {});
                }
                Item::File(id) => {
                    let f = r.get(*id).unwrap();
                    row.col(|ui| {
                        let (icon, color) = if f.category() == Category::Video { (ic::FILM_STRIP, p.video) } else { (ic::IMAGE, p.photo) };
                        theme::icon_tile(ui, icon, color, 28.0);
                        ui.label(txt(&f.name, 14.0, W::Medium, p.text));
                    });
                    row.col(|ui| {
                        ui.label(txt(format_size(f.size), 13.5, W::Regular, p.muted));
                    });
                    row.col(|ui| {
                        ui.label(txt(f.date().map(format_date).unwrap_or_default(), 13.5, W::Regular, p.muted));
                    });
                    row.col(|ui| {
                        ui.label(txt(f.format.name(), 13.5, W::Regular, p.muted));
                    });
                    row.col(|ui| {
                        theme::health_badge(ui, f.health);
                    });
                }
            }
            let resp = row.response();
            if resp.double_clicked() {
                clicks.push((k, false, true));
            } else if resp.clicked() {
                clicks.push((k, false, false));
            }
        });
    });
    for (k, cb, dbl) in clicks {
        if dbl {
            r.activate(k);
            break;
        }
        r.click(k, mods, cb);
    }
}

// ---------------------------------------------------------------- action bar

fn action_bar(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    egui::Panel::bottom("actions")
        .frame(egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).inner_margin(egui::Margin::symmetric(24, 12)))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let r = &app.results;
                let n = r.checked.len();
                let bytes: u64 = r.checked.iter().filter_map(|id| r.get(*id)).map(|f| f.size).sum();
                if n == 0 {
                    ui.label(txt("Nothing selected", 15.0, W::Medium, p.text));
                    if ui.available_width() > 760.0 {
                        ui.label(txt("Tick files or whole folders. Shift selects a range, Ctrl adds one.", 14.0, W::Regular, p.faint));
                    }
                } else {
                    ui.label(txt(format!("{} selected", n), 15.0, W::Semibold, p.text));
                    ui.label(txt(format_size(bytes), 14.0, W::Regular, p.muted));
                    let over = r.checked.iter().filter_map(|id| r.get(*id)).filter(|f| f.health == Health::Overwritten).count();
                    if over > 0 {
                        theme::chip(ui, &format!("{} overwritten", over), p.bad);
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let n = app.results.checked.len();
                    if theme::primary_button(ui, ic::DOWNLOAD_SIMPLE, &if n > 0 { format!("Recover {}", n) } else { "Recover".to_string() }, n > 0).clicked() {
                        let files = app.results.checked_files();
                        crate::recovery::open_dialog(app, files);
                    }
                    if n > 0 && theme::ghost_button(ui, "", "Clear").clicked() {
                        app.results.checked.clear();
                    }
                });
            });
        });
}
