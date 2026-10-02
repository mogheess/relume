//! Scan orchestration: layout detection -> quick (file-system) scan -> deep (carving) scan ->
//! lost-partition recovery. Streams results through a channel so the UI updates live.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::carve::{self, CarveOptions};
use crate::device::Dev;
use crate::formats::{Category, Format, Probe};
use crate::fs::{self, AllocMap, FsContext, FsKind};
use crate::model::{Extent, FileState, FoundFile, Health, Origin};
use crate::partition::{self, VolumeRegion};
use crate::util;

#[derive(Clone, Debug)]
pub enum FolderFilter {
    /// Volume-relative folder, e.g. "\\Users\\me\\Pictures".
    Path(String),
    RecycleBin,
}

#[derive(Clone, Debug)]
pub struct ScanRequest {
    pub quick: bool,
    pub deep: bool,
    pub categories: Vec<Category>,
    pub folder: Option<FolderFilter>,
    /// Deep scan only free space when the file system is intact (much faster).
    pub deep_free_only: bool,
    pub skip_inside_found: bool,
}

impl Default for ScanRequest {
    fn default() -> Self {
        ScanRequest {
            quick: true,
            deep: true,
            categories: vec![Category::Image, Category::Video],
            folder: None,
            deep_free_only: true,
            skip_inside_found: true,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ScanEvent {
    Phase(String),
    Progress { fraction: f32, scanned: u64, total: u64 },
    Volumes(Vec<VolumeRegion>),
    Found(Vec<FoundFile>),
    /// Occupancy of the whole device in equal buckets (0 = free, 255 = full), for the map.
    SpaceMap(Vec<u8>),
    Warning(String),
    Finished { cancelled: bool, secs: f64, bad_sectors: u32 },
}

fn matches_folder(f: &FoundFile, filter: &Option<FolderFilter>) -> bool {
    match filter {
        None => true,
        Some(FolderFilter::RecycleBin) => f.state == FileState::RecycleBin,
        Some(FolderFilter::Path(p)) => {
            let want = p.trim_end_matches('\\').to_lowercase();
            let have = f.path.to_lowercase();
            want.is_empty() || have == want || have.starts_with(&(want + "\\"))
        }
    }
}

struct Batcher<'a> {
    tx: &'a Sender<ScanEvent>,
    buf: Vec<FoundFile>,
    last: Instant,
}

impl Batcher<'_> {
    fn push(&mut self, f: FoundFile) {
        self.buf.push(f);
        if self.buf.len() >= 256 || self.last.elapsed().as_millis() > 300 {
            self.flush();
        }
    }
    fn flush(&mut self) {
        if !self.buf.is_empty() {
            let _ = self.tx.send(ScanEvent::Found(std::mem::take(&mut self.buf)));
        }
        self.last = Instant::now();
    }
}

pub fn run_scan(dev: Dev, req: ScanRequest, tx: Sender<ScanEvent>, cancel: Arc<AtomicBool>) {
    let t0 = Instant::now();
    let total = dev.len();
    let mut bad = 0u32;
    let mut regions = partition::layout(&dev);
    let _ = tx.send(ScanEvent::Volumes(regions.clone()));
    let (qw, dw) = match (req.quick, req.deep) {
        (true, true) => (0.08f32, 0.92f32),
        (true, false) => (1.0, 0.0),
        _ => (0.0, 1.0),
    };
    let mut batch = Batcher { tx: &tx, buf: Vec::new(), last: Instant::now() };
    let mut allocs: Vec<AllocMap> = Vec::new();

    if req.quick {
        let with_fs: Vec<&VolumeRegion> = regions.iter().filter(|r| r.fs.is_some()).collect();
        if with_fs.is_empty() {
            let _ = tx.send(ScanEvent::Warning(
                "No readable file system found (formatted, RAW or damaged). Deep scan will search the raw data.".into(),
            ));
        }
        let n = with_fs.len().max(1) as f32;
        for (i, reg) in with_fs.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let _ = tx.send(ScanEvent::Phase(format!("Reading {} file records", reg.fs.unwrap().label())));
            let txp = Mutex::new(tx.clone());
            let prog = move |p: f32| {
                let _ = txp.lock().unwrap().send(ScanEvent::Progress { fraction: qw * (i as f32 + p) / n, scanned: 0, total });
            };
            let ctx = FsContext { dev: dev.clone(), vol_off: reg.offset, vol_len: reg.size, categories: &req.categories, cancel: &cancel, progress: &prog };
            match fs::scan(&ctx, reg.fs.unwrap()) {
                Ok(res) => {
                    for w in res.warnings {
                        let _ = tx.send(ScanEvent::Warning(w));
                    }
                    for mut f in res.files {
                        if f.volume.is_empty() {
                            f.volume = reg.name.clone();
                        }
                        if matches_folder(&f, &req.folder) || f.state == FileState::Existing {
                            batch.push(f);
                        }
                    }
                    if let Some(a) = res.alloc {
                        allocs.push(a);
                    }
                }
                Err(e) => {
                    let _ = tx.send(ScanEvent::Warning(format!("{}: {}", reg.describe(), e)));
                }
            }
        }
        batch.flush();
    }

    if !allocs.is_empty() {
        let _ = tx.send(ScanEvent::SpaceMap(space_map(&allocs, total, 2048)));
    }

    if req.deep && !cancel.load(Ordering::Relaxed) {
        let _ = tx.send(ScanEvent::Phase("Deep scan: reading every sector".into()));
        let free_only = req.deep_free_only && !allocs.is_empty();
        let skip = |pos: u64| -> Option<u64> {
            if !free_only {
                return None;
            }
            for a in &allocs {
                if a.contains(pos) {
                    return a.allocated(pos).then(|| a.next_free(pos));
                }
            }
            None
        };
        let mut counters: HashMap<Format, u32> = HashMap::new();
        let regs = regions.clone();
        let base_frac = qw;
        let txp = tx.clone();
        // Testing aid: RELUME_THROTTLE_MS slows the deep scan so its live view can be inspected.
        let throttle = if cfg!(feature = "dev-hooks") { std::env::var("RELUME_THROTTLE_MS").ok().and_then(|v| v.parse::<u64>().ok()) } else { None };
        let mut on_progress = |p: u64| {
            if let Some(ms) = throttle {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
            let _ = txp.send(ScanEvent::Progress { fraction: base_frac + dw * (p as f32 / total.max(1) as f32), scanned: p, total });
        };
        let mut on_found = |off: u64, pr: Probe| {
            let c = counters.entry(pr.format).or_insert(0);
            *c += 1;
            let vol = regs.iter().find(|r| off >= r.offset && off < r.offset + r.size).map(|r| r.name.clone()).unwrap_or_default();
            batch.push(carved_file(off, pr, *c, vol));
        };
        let opts = CarveOptions { categories: &req.categories, skip_found: req.skip_inside_found };
        let stats = carve::carve(&dev, 0, total, &opts, &skip, &cancel, &mut on_progress, &mut on_found);
        bad += stats.bad_sectors;
        batch.flush();

        // Lost / deleted partitions: boot sectors that aren't part of the current layout.
        if !cancel.load(Ordering::Relaxed) {
            let mut lost: Vec<VolumeRegion> = Vec::new();
            for at in stats.boot_sectors {
                let Some((start, _)) = partition::resolve_boot(&dev, at) else { continue };
                if regions.iter().chain(lost.iter()).any(|r| r.offset == start) {
                    continue;
                }
                let reg = partition::region_at(&dev, start, 0, &format!("Lost partition @ {}", util::format_size(start)), true);
                if reg.fs.is_some() && reg.size > 1 << 20 {
                    lost.push(reg);
                }
            }
            if !lost.is_empty() {
                let _ = tx.send(ScanEvent::Phase(format!("Restoring names from {} lost partition(s)", lost.len())));
                for reg in &lost {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let noop = |_p: f32| {};
                    let ctx = FsContext { dev: dev.clone(), vol_off: reg.offset, vol_len: reg.size, categories: &req.categories, cancel: &cancel, progress: &noop };
                    if let Ok(res) = fs::scan(&ctx, reg.fs.unwrap()) {
                        for mut f in res.files {
                            // Everything on a lost partition is lost to the user, even "existing" entries.
                            if f.state == FileState::Existing {
                                f.state = FileState::Deleted;
                                if f.health == Health::Good {
                                    let v = fs::verify(&dev, &f.extents, f.size, f.format, false);
                                    fs::apply_verification(&mut f, v, 0.0);
                                }
                            }
                            f.volume = reg.name.clone();
                            batch.push(f);
                        }
                    }
                }
                batch.flush();
                regions.extend(lost);
                let _ = tx.send(ScanEvent::Volumes(regions.clone()));
            }
        }

        // Orphaned FAT folders: directory clusters whose parent entry is gone or overwritten.
        if !cancel.load(Ordering::Relaxed) && !stats.fat_dirs.is_empty() {
            for reg in regions.iter().filter(|r| matches!(r.fs, Some(FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32))) {
                let inside: Vec<u64> = stats.fat_dirs.iter().copied().filter(|&o| o >= reg.offset && o < reg.offset + reg.size).collect();
                if inside.is_empty() {
                    continue;
                }
                let _ = tx.send(ScanEvent::Phase("Rebuilding orphaned folders".into()));
                let noop = |_p: f32| {};
                let ctx = FsContext { dev: dev.clone(), vol_off: reg.offset, vol_len: reg.size, categories: &req.categories, cancel: &cancel, progress: &noop };
                for mut f in fs::fat::scan_orphans(&ctx, &inside) {
                    if f.volume.trim().is_empty() {
                        f.volume = reg.name.clone();
                    }
                    batch.push(f);
                }
                batch.flush();
            }
        }
    }
    let _ = tx.send(ScanEvent::Progress { fraction: 1.0, scanned: total, total });
    let _ = tx.send(ScanEvent::Finished { cancelled: cancel.load(Ordering::Relaxed), secs: t0.elapsed().as_secs_f64(), bad_sectors: bad });
}

fn space_map(allocs: &[AllocMap], total: u64, n: usize) -> Vec<u8> {
    (0..n as u64)
        .map(|i| {
            let (a, b) = (i * total / n as u64, (i + 1) * total / n as u64);
            let mut used = 0.0f32;
            for m in allocs {
                if a < m.vol_end && b > m.vol_start {
                    let ov = (b.min(m.vol_end) - a.max(m.vol_start)) as f32 / (b - a).max(1) as f32;
                    used += m.used_fraction(a, b) * ov;
                }
            }
            (used.min(1.0) * 255.0) as u8
        })
        .collect()
}

fn carved_file(off: u64, pr: Probe, n: u32, volume: String) -> FoundFile {
    let fmt = pr.format;
    let base = match pr.info.taken {
        Some(t) => format!("{}_{}", if fmt.category() == Category::Video { "VID" } else { "IMG" }, util::format_date_compact(t)),
        None => format!("{}_{:05}", fmt.ext().to_uppercase(), n),
    };
    let complete = pr.complete;
    FoundFile {
        id: 0,
        name: format!("{}.{}", base, fmt.ext()),
        path: format!("\\Lost Files\\{}", fmt.name()),
        volume,
        format: fmt,
        size: pr.len,
        extents: vec![Extent::at(off, pr.len)],
        origin: Origin::Carved,
        state: FileState::Lost,
        health: if complete { Health::Excellent } else { Health::Damaged },
        note: pr.note.unwrap_or_default(),
        modified: None,
        created: None,
        info: pr.info,
        fix_jpeg_eoi: !complete && fmt == Format::Jpeg,
    }
}
