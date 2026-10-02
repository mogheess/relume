//! File-system metadata scanners (the "quick scan"): recover names, folders and dates of
//! deleted files from NTFS / FAT / exFAT, and verify each one against the bytes on disk.

pub mod exfat;
pub mod fat;
pub mod ntfs;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::device::{Dev, ExtentDevice, Reader};
use crate::formats::{self, Category, Family, Format, MediaInfo};
use crate::model::{Extent, FoundFile, Health};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FsKind {
    Ntfs,
    Fat12,
    Fat16,
    Fat32,
    ExFat,
}

impl FsKind {
    pub fn label(self) -> &'static str {
        match self {
            FsKind::Ntfs => "NTFS",
            FsKind::Fat12 => "FAT12",
            FsKind::Fat16 => "FAT16",
            FsKind::Fat32 => "FAT32",
            FsKind::ExFat => "exFAT",
        }
    }
}

/// Which clusters of a volume are in use. Lets the deep scan skip live data.
pub struct AllocMap {
    /// Absolute offset of the volume.
    pub vol_start: u64,
    pub vol_end: u64,
    /// Absolute offset where cluster `first_cluster` begins.
    pub heap_start: u64,
    pub first_cluster: u64,
    pub cluster: u64,
    /// One bit per cluster starting at `first_cluster`.
    pub bits: Vec<u64>,
    pub clusters: u64,
}

impl AllocMap {
    pub fn new(vol_start: u64, vol_end: u64, heap_start: u64, first_cluster: u64, cluster: u64, clusters: u64) -> AllocMap {
        AllocMap { vol_start, vol_end, heap_start, first_cluster, cluster, bits: vec![0; clusters.div_ceil(64) as usize], clusters }
    }
    pub fn set(&mut self, idx: u64) {
        if idx < self.clusters {
            self.bits[(idx / 64) as usize] |= 1 << (idx % 64);
        }
    }
    fn bit(&self, idx: u64) -> bool {
        idx < self.clusters && self.bits[(idx / 64) as usize] & (1 << (idx % 64)) != 0
    }
    pub fn contains(&self, abs: u64) -> bool {
        abs >= self.vol_start && abs < self.vol_end
    }
    /// Is this absolute offset part of live data or file-system metadata?
    pub fn allocated(&self, abs: u64) -> bool {
        if !self.contains(abs) {
            return false;
        }
        if abs < self.heap_start {
            return true;
        }
        let idx = (abs - self.heap_start) / self.cluster;
        if idx >= self.clusters {
            return false;
        }
        self.bit(idx)
    }
    /// First unallocated offset >= abs (within the volume), or vol_end.
    pub fn next_free(&self, abs: u64) -> u64 {
        if abs < self.heap_start {
            return self.next_free(self.heap_start);
        }
        let mut idx = (abs - self.heap_start) / self.cluster;
        while idx < self.clusters {
            let w = self.bits[(idx / 64) as usize];
            if w == u64::MAX && idx % 64 == 0 {
                idx += 64;
                continue;
            }
            if !self.bit(idx) {
                let at = self.heap_start + idx * self.cluster;
                return at.max(abs);
            }
            idx += 1;
        }
        self.vol_end
    }
    /// First allocated offset >= abs, or vol_end.
    pub fn next_allocated(&self, abs: u64) -> u64 {
        if abs < self.heap_start {
            return abs;
        }
        let mut idx = (abs - self.heap_start) / self.cluster;
        while idx < self.clusters {
            let w = self.bits[(idx / 64) as usize];
            if w == 0 && idx % 64 == 0 {
                idx += 64;
                continue;
            }
            if self.bit(idx) {
                return (self.heap_start + idx * self.cluster).max(abs);
            }
            idx += 1;
        }
        self.vol_end
    }
    /// Approximate fraction of `[start, end)` (absolute offsets) holding live data.
    pub fn used_fraction(&self, start: u64, end: u64) -> f32 {
        let (s, e) = (start.max(self.vol_start), end.min(self.vol_end));
        if e <= s {
            return 0.0;
        }
        let meta = self.heap_start.min(e).saturating_sub(s);
        let hs = s.max(self.heap_start);
        if e <= hs {
            return 1.0;
        }
        let c0 = (hs - self.heap_start) / self.cluster;
        let c1 = ((e - self.heap_start).div_ceil(self.cluster)).min(self.clusters);
        let mut used = 0u64;
        let mut c = c0;
        while c < c1 {
            if c % 64 == 0 && c + 64 <= c1 {
                used += self.bits[(c / 64) as usize].count_ones() as u64;
                c += 64;
            } else {
                used += self.bit(c) as u64;
                c += 1;
            }
        }
        let used_bytes = meta as f64 + used as f64 * self.cluster as f64;
        (used_bytes / (e - s) as f64).min(1.0) as f32
    }

    /// Fraction of the given extents that is currently allocated to something else.
    pub fn overlap(&self, extents: &[Extent]) -> f64 {
        let (mut total, mut used) = (0u64, 0u64);
        for e in extents {
            let Some(o) = e.offset else { continue };
            if e.len == 0 || o < self.heap_start || !self.contains(o) {
                continue;
            }
            let first = (o - self.heap_start) / self.cluster;
            let last = (o + e.len - 1 - self.heap_start) / self.cluster;
            for idx in first..=last.min(first + 4_000_000) {
                total += 1;
                if self.bit(idx) {
                    used += 1;
                }
            }
        }
        if total == 0 { 0.0 } else { used as f64 / total as f64 }
    }
}

pub struct FsScan {
    pub kind: FsKind,
    pub label: String,
    pub files: Vec<FoundFile>,
    pub alloc: Option<AllocMap>,
    pub warnings: Vec<String>,
}

pub struct FsContext<'a> {
    pub dev: Dev,
    /// Absolute offset of the volume within `dev`.
    pub vol_off: u64,
    pub vol_len: u64,
    pub categories: &'a [Category],
    pub cancel: &'a AtomicBool,
    pub progress: &'a (dyn Fn(f32) + Sync),
}

impl FsContext<'_> {
    pub fn wants(&self, f: Format) -> bool {
        self.categories.contains(&f.category())
    }
}

/// Identify a volume boot sector.
pub fn detect(b: &[u8]) -> Option<FsKind> {
    if b.len() < 512 {
        return None;
    }
    if &b[3..11] == b"NTFS    " && ntfs::Boot::parse(b).is_some() {
        return Some(FsKind::Ntfs);
    }
    if &b[3..11] == b"EXFAT   " && exfat::Boot::parse(b).is_some() {
        return Some(FsKind::ExFat);
    }
    fat::Boot::parse(b).map(|f| f.kind)
}

/// Volume size declared by a boot sector.
pub fn volume_size(b: &[u8]) -> Option<u64> {
    match detect(b)? {
        FsKind::Ntfs => ntfs::Boot::parse(b).map(|x| x.total_bytes()),
        FsKind::ExFat => exfat::Boot::parse(b).map(|x| x.total_bytes()),
        _ => fat::Boot::parse(b).map(|x| x.total_bytes()),
    }
}

pub fn scan(ctx: &FsContext, kind: FsKind) -> Result<FsScan, String> {
    match kind {
        FsKind::Ntfs => ntfs::scan(ctx),
        FsKind::ExFat => exfat::scan(ctx),
        _ => fat::scan(ctx),
    }
}

pub struct Verification {
    pub health: Health,
    pub format: Format,
    pub info: MediaInfo,
    pub note: String,
    pub complete: bool,
}

/// Check what is really stored at a deleted file's location.
/// `thorough` walks the full structure (reads the whole file); otherwise cheap checks only.
pub fn verify(dev: &Dev, extents: &[Extent], size: u64, expected: Format, thorough: bool) -> Verification {
    let ed: Dev = Arc::new(ExtentDevice::new(dev.clone(), extents.to_vec(), size));
    let mut r = Reader::with_capacity(ed, 8);
    verify_reader(&mut r, size, expected, thorough)
}

pub fn verify_reader(r: &mut Reader, size: u64, expected: Format, thorough: bool) -> Verification {
    let mut v = Verification { health: Health::Overwritten, format: expected, info: MediaInfo::default(), note: String::new(), complete: false };
    let head = r.bytes(0, 512);
    if head.is_empty() {
        v.note = "Unreadable".into();
        return v;
    }
    if head.iter().all(|&b| b == 0) {
        v.note = "Data wiped (zeroed by SSD TRIM or overwritten)".into();
        return v;
    }
    let Some(found) = formats::identify(&head) else {
        v.note = "Overwritten by other data".into();
        return v;
    };
    let fam = found.family();
    if fam != expected.family() {
        // Data is a valid file of another type: still recoverable, just mislabeled.
        v.note = format!("Contains {} data", found.name());
    }
    v.format = if fam == expected.family() && found.family() != Family::Tiff && found.family() != Family::Iso { expected } else { found };
    let cheap = matches!(fam, Family::Iso | Family::Tiff | Family::Riff | Family::Asf | Family::Bmp | Family::Psd | Family::Raf);
    if thorough || cheap {
        match formats::probe_as(r, 0, size, found) {
            Some(p) => {
                v.format = p.format;
                v.info = p.info;
                v.complete = p.complete;
                if p.complete {
                    v.health = if thorough || fam == Family::Iso { Health::Excellent } else { Health::Good };
                    if p.len + 4096 < size && fam != Family::Iso {
                        v.note = "Valid image, followed by extra data".into();
                    }
                } else {
                    v.health = Health::Damaged;
                    v.note = p.note.unwrap_or_else(|| "Partially overwritten".into());
                }
            }
            None => {
                v.health = Health::Damaged;
                v.note = "Header present but structure damaged".into();
            }
        }
        return v;
    }
    // Cheap checks for stream formats: header + sampled interior sectors + tail marker.
    v.health = Health::Good;
    if fam == Family::Jpeg {
        if let Some(p) = header_only_jpeg(r, size) {
            v.info = p;
        }
    }
    if size > 4096 {
        for frac in [0.25f64, 0.5, 0.75] {
            let at = ((size as f64 * frac) as u64) / 512 * 512;
            let s = r.bytes(at, 512);
            if s.len() == 512 && formats::sector_break(&s) {
                v.health = Health::Damaged;
                v.note = format!("Partially overwritten (~{}% intact)", (frac * 100.0) as u32);
                return v;
            }
        }
    }
    let tail_ok = match fam {
        Family::Jpeg => {
            let t = r.bytes(size.saturating_sub(4096), 4096.min(size as usize));
            t.windows(2).rev().take(4096).any(|w| w == [0xFF, 0xD9])
        }
        Family::Png => {
            let t = r.bytes(size.saturating_sub(12), 12);
            t.ends_with(&[0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82])
        }
        Family::Gif => r.u8(size.saturating_sub(1)) == Some(0x3B),
        _ => true,
    };
    if !tail_ok {
        v.health = Health::Damaged;
        v.note = "End of file missing or overwritten".into();
    }
    v
}

/// Parse JPEG headers (dimensions, EXIF) without walking the scan data.
fn header_only_jpeg(r: &mut Reader, size: u64) -> Option<MediaInfo> {
    let mut info = MediaInfo::default();
    let mut pos = 2u64;
    for _ in 0..64 {
        if pos + 4 > size || r.u8(pos)? != 0xFF {
            break;
        }
        let m = r.u8(pos + 1)?;
        let len = r.u16be(pos + 2)? as u64;
        if matches!(m, 0xC0..=0xCF) && m != 0xC4 && m != 0xC8 && m != 0xCC {
            info.height = r.u16be(pos + 5).map(|v| v as u32);
            info.width = r.u16be(pos + 7).map(|v| v as u32);
        } else if m == 0xE1 && r.bytes(pos + 4, 6) == b"Exif\0\0" {
            let t = formats::tiff::walk(r, pos + 10, len.saturating_sub(8));
            info.taken = t.date;
            info.camera = t.camera();
        }
        if m == 0xDA {
            break;
        }
        pos += 2 + len;
    }
    Some(info)
}

/// Merge a list of (absolute offset, length) runs into extents, joining adjacent ones.
pub fn merge_runs(runs: impl IntoIterator<Item = (Option<u64>, u64)>, size: u64) -> Vec<Extent> {
    let mut out: Vec<Extent> = Vec::new();
    let mut total = 0u64;
    for (off, len) in runs {
        if total >= size {
            break;
        }
        let len = len.min(size - total);
        total += len;
        if let Some(last) = out.last_mut() {
            match (last.offset, off) {
                (Some(a), Some(b)) if a + last.len == b => {
                    last.len += len;
                    continue;
                }
                (None, None) => {
                    last.len += len;
                    continue;
                }
                _ => {}
            }
        }
        out.push(Extent { offset: off, len });
    }
    out
}

/// Format for a file name worth recovering. macOS "._name" AppleDouble companions
/// (metadata, not media) are ignored.
pub fn media_name(name: &str) -> Option<Format> {
    if name.starts_with("._") {
        return None;
    }
    format_for_name(name)
}

/// Split "name.ext" -> Format by extension.
pub fn format_for_name(name: &str) -> Option<Format> {
    let ext = name.rsplit_once('.')?.1;
    Format::from_ext(ext)
}

/// Fill in verification results for a deleted file found in metadata.
pub fn apply_verification(f: &mut FoundFile, v: Verification, overlap: f64) {
    f.health = v.health;
    f.format = v.format;
    if !v.info.is_empty() {
        f.info = v.info;
    }
    f.note = v.note;
    if overlap > 0.0 && f.health != Health::Overwritten {
        if f.health == Health::Excellent && v.complete {
            // Structure validated end to end despite re-allocation: data intact for now.
            f.note = format!("Space reused by another file ({:.0}%) but data still intact", overlap * 100.0);
        } else {
            f.health = Health::Damaged;
            f.note = format!("{:.0}% of its space now belongs to another file", overlap * 100.0);
        }
    }
    if f.health == Health::Damaged && f.format == Format::Jpeg {
        f.fix_jpeg_eoi = true;
    }
}
