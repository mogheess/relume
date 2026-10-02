//! FAT12/16/32: walk live and deleted directories, rebuild long file names, and recover
//! deleted files whose cluster chains were wiped. Unlike simple undeleters, each candidate
//! layout (contiguous / skip-allocated / FAT32 high-word guesses) is checked against the
//! real bytes and the one that validates wins.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::Ordering;

use super::{apply_verification, media_name, merge_runs, verify, AllocMap, FsContext, FsKind, FsScan};
use crate::device::{read_tolerant, Dev};
use crate::formats::{self, Format};
use crate::model::{Extent, FileState, FoundFile, Health, Origin};
use crate::util::{dos_to_unix, le16, le32};

pub struct Boot {
    pub kind: FsKind,
    pub bps: u64,
    pub cluster: u64,
    pub reserved: u64,
    pub nfats: u64,
    pub fat_sectors: u64,
    pub root_entries: u64,
    pub total_sectors: u64,
    pub root_cluster: u32,
    pub clusters: u64,
    pub label: String,
}

impl Boot {
    pub fn parse(b: &[u8]) -> Option<Boot> {
        if b.len() < 512 || b[510] != 0x55 || b[511] != 0xAA {
            return None;
        }
        if !(b[0] == 0xEB || b[0] == 0xE9) {
            return None;
        }
        let bps = le16(b, 0x0B) as u64;
        let spc = b[0x0D] as u64;
        if !matches!(bps, 512 | 1024 | 2048 | 4096) || spc == 0 || !spc.is_power_of_two() {
            return None;
        }
        let reserved = le16(b, 0x0E) as u64;
        let nfats = b[0x10] as u64;
        let root_entries = le16(b, 0x11) as u64;
        let total16 = le16(b, 0x13) as u64;
        let fat16 = le16(b, 0x16) as u64;
        let total_sectors = if total16 != 0 { total16 } else { le32(b, 0x20) as u64 };
        let fat_sectors = if fat16 != 0 { fat16 } else { le32(b, 0x24) as u64 };
        if reserved == 0 || !(1..=2).contains(&nfats) || fat_sectors == 0 || total_sectors == 0 {
            return None;
        }
        let root_sectors = (root_entries * 32).div_ceil(bps);
        let meta = reserved + nfats * fat_sectors + root_sectors;
        if meta >= total_sectors {
            return None;
        }
        let clusters = (total_sectors - meta) / spc;
        let kind = if clusters < 4085 {
            FsKind::Fat12
        } else if clusters < 65525 {
            FsKind::Fat16
        } else {
            FsKind::Fat32
        };
        if kind == FsKind::Fat32 && (root_entries != 0 || fat16 != 0) {
            return None;
        }
        if kind != FsKind::Fat32 && root_entries == 0 {
            return None;
        }
        let label_at = if kind == FsKind::Fat32 { 0x47 } else { 0x2B };
        let sig_at = if kind == FsKind::Fat32 { 0x42 } else { 0x26 };
        let label = if b[sig_at] == 0x29 { crate::util::ascii_trim(&b[label_at..label_at + 11]) } else { String::new() };
        Some(Boot {
            kind,
            bps,
            cluster: bps * spc,
            reserved,
            nfats,
            fat_sectors,
            root_entries,
            total_sectors,
            root_cluster: if kind == FsKind::Fat32 { le32(b, 0x2C) } else { 0 },
            clusters,
            label: if label == "NO NAME" { String::new() } else { label },
        })
    }
    pub fn total_bytes(&self) -> u64 {
        self.total_sectors * self.bps
    }
    fn data_start(&self) -> u64 {
        (self.reserved + self.nfats * self.fat_sectors) * self.bps + self.root_entries * 32
    }
}

struct Fat {
    entries: Vec<u32>,
    eoc: u32,
}

impl Fat {
    fn get(&self, c: u32) -> u32 {
        self.entries.get(c as usize).copied().unwrap_or(0)
    }
    fn chain(&self, start: u32, max: u64) -> Vec<u32> {
        let mut out = Vec::new();
        let mut c = start;
        while c >= 2 && (c as usize) < self.entries.len() && (out.len() as u64) < max {
            out.push(c);
            let n = self.get(c);
            if n >= self.eoc || n < 2 || n == c {
                break;
            }
            c = n;
        }
        out
    }
}

struct Ctx<'a> {
    dev: &'a Dev,
    vol_off: u64,
    boot: Boot,
    fat: Fat,
    data_start: u64,
}

impl Ctx<'_> {
    fn cluster_off(&self, c: u32) -> u64 {
        self.vol_off + self.data_start + (c as u64 - 2) * self.boot.cluster
    }
    fn valid(&self, c: u32) -> bool {
        c >= 2 && (c as u64) < self.boot.clusters + 2
    }
    fn read_clusters(&self, cl: &[u32]) -> Vec<u8> {
        let cs = self.boot.cluster as usize;
        let mut out = vec![0u8; cl.len() * cs];
        for (i, &c) in cl.iter().enumerate() {
            read_tolerant(&**self.dev, self.cluster_off(c), &mut out[i * cs..(i + 1) * cs]);
        }
        out
    }
    fn head(&self, c: u32) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        read_tolerant(&**self.dev, self.cluster_off(c), &mut b);
        b
    }
}

struct Entry {
    name: String,
    /// Deleted 8.3 entry without a long name: first character is unknown.
    lost_first: bool,
    deleted: bool,
    is_dir: bool,
    cluster: u32,
    size: u32,
    ctime: Option<i64>,
    mtime: Option<i64>,
}

fn sfn_checksum(n: &[u8]) -> u8 {
    n.iter().fold(0u8, |s, &c| s.rotate_right(1).wrapping_add(c))
}

fn parse_dir(data: &[u8], label: &mut String) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut lfn: Vec<(u8, u8, Vec<u16>)> = Vec::new();
    for e in data.chunks_exact(32) {
        let b0 = e[0];
        if b0 == 0 {
            break;
        }
        let attr = e[11];
        if attr == 0x0F {
            let mut chars = Vec::with_capacity(13);
            for r in [1..11, 14..26, 28..32] {
                for c in e[r].chunks_exact(2) {
                    chars.push(u16::from_le_bytes([c[0], c[1]]));
                }
            }
            lfn.push((b0, e[13], chars));
            continue;
        }
        let parts = std::mem::take(&mut lfn);
        if attr & 0x08 != 0 {
            if b0 != 0xE5 && attr & 0x10 == 0 && label.is_empty() {
                *label = crate::util::ascii_trim(&e[..11]);
            }
            continue;
        }
        if b0 == b'.' {
            continue;
        }
        let deleted = b0 == 0xE5;
        let mut sfn: [u8; 11] = e[..11].try_into().unwrap();
        if sfn[0] == 0x05 {
            sfn[0] = 0xE5;
        }
        let mut name = None;
        if !parts.is_empty() {
            let cks = parts[0].1;
            let consistent = parts.iter().all(|p| p.1 == cks);
            let matches = if deleted {
                // First SFN byte is lost; try the LFN's first letter.
                let first = parts.last().and_then(|p| p.2.first()).copied().unwrap_or(0);
                let mut s = sfn;
                s[0] = (first as u8).to_ascii_uppercase();
                sfn_checksum(&s) == cks || parts.iter().all(|p| p.0 == 0xE5)
            } else {
                sfn_checksum(&sfn) == cks
            };
            if consistent && matches {
                let mut u: Vec<u16> = Vec::new();
                for p in parts.iter().rev() {
                    u.extend_from_slice(&p.2);
                }
                let end = u.iter().position(|&c| c == 0 || c == 0xFFFF).unwrap_or(u.len());
                let s = String::from_utf16_lossy(&u[..end]);
                if !s.is_empty() {
                    name = Some(s);
                }
            }
        }
        let lost_first = deleted && name.is_none();
        let name = name.unwrap_or_else(|| {
            let lower_base = e[12] & 0x08 != 0;
            let lower_ext = e[12] & 0x10 != 0;
            let mut base: String = sfn[..8].iter().map(|&c| c as char).collect::<String>().trim_end().to_string();
            let mut ext: String = sfn[8..].iter().map(|&c| c as char).collect::<String>().trim_end().to_string();
            if deleted {
                base.replace_range(..base.chars().next().map(|c| c.len_utf8()).unwrap_or(0), "_");
            }
            if lower_base {
                base = base.to_lowercase();
            }
            if lower_ext {
                ext = ext.to_lowercase();
            }
            if ext.is_empty() { base } else { format!("{}.{}", base, ext) }
        });
        out.push(Entry {
            name,
            lost_first,
            deleted,
            is_dir: attr & 0x10 != 0,
            cluster: ((le16(e, 0x14) as u32) << 16) | le16(e, 0x1A) as u32,
            size: le32(e, 0x1C),
            ctime: dos_to_unix(le16(e, 0x10), le16(e, 0x0E)),
            mtime: dos_to_unix(le16(e, 0x18), le16(e, 0x16)),
        });
    }
    out
}

fn read_fat(dev: &Dev, vol_off: u64, boot: &Boot) -> Fat {
    let fat_off = vol_off + boot.reserved * boot.bps;
    let n = boot.clusters as usize + 2;
    let (bytes, eoc) = match boot.kind {
        FsKind::Fat12 => ((n * 3).div_ceil(2), 0xFF8),
        FsKind::Fat16 => (n * 2, 0xFFF8),
        _ => (n * 4, 0x0FFF_FFF8),
    };
    let bytes = bytes.min((boot.fat_sectors * boot.bps) as usize);
    let mut raw = vec![0u8; bytes];
    read_tolerant(&**dev, fat_off, &mut raw);
    let entries = match boot.kind {
        FsKind::Fat12 => (0..n)
            .map(|i| {
                let o = i * 3 / 2;
                if o + 1 >= raw.len() {
                    return 0;
                }
                let v = u16::from_le_bytes([raw[o], raw[o + 1]]) as u32;
                if i % 2 == 0 { v & 0xFFF } else { v >> 4 }
            })
            .collect(),
        FsKind::Fat16 => raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32).collect(),
        _ => raw.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) & 0x0FFF_FFFF).collect(),
    };
    Fat { entries, eoc }
}

fn open<'a>(ctx: &'a FsContext) -> Result<(Ctx<'a>, AllocMap), String> {
    let dev = &ctx.dev;
    let mut bs = vec![0u8; 512];
    read_tolerant(&**dev, ctx.vol_off, &mut bs);
    let boot = Boot::parse(&bs).ok_or("Not a FAT boot sector")?;
    let fat = read_fat(dev, ctx.vol_off, &boot);
    let data_start = boot.data_start();
    let c = Ctx { dev, vol_off: ctx.vol_off, data_start, fat, boot };
    let mut alloc = AllocMap::new(ctx.vol_off, ctx.vol_off + ctx.vol_len, ctx.vol_off + data_start, 2, c.boot.cluster, c.boot.clusters);
    for i in 0..c.boot.clusters {
        if c.fat.get(i as u32 + 2) != 0 {
            alloc.set(i);
        }
    }
    Ok((c, alloc))
}

fn root_dir(c: &Ctx) -> Vec<u8> {
    if c.boot.kind == FsKind::Fat32 {
        c.read_clusters(&c.fat.chain(c.boot.root_cluster, 1 << 16))
    } else {
        let mut b = vec![0u8; (c.boot.root_entries * 32) as usize];
        read_tolerant(&**c.dev, c.vol_off + (c.boot.reserved + c.boot.nfats * c.boot.fat_sectors) * c.boot.bps, &mut b);
        b
    }
}

struct Walk<'a> {
    c: &'a Ctx<'a>,
    alloc: &'a AllocMap,
    ctx: &'a FsContext<'a>,
    label: String,
    seen_dirs: HashSet<u32>,
    files: Vec<FoundFile>,
    collect: bool,
}

impl Walk<'_> {
    fn run(&mut self, start: Vec<(Vec<u8>, String, bool)>) -> Result<(), String> {
        let c = self.c;
        let cs = c.boot.cluster;
        let mut queue: VecDeque<(Vec<u8>, String, bool)> = start.into();
        let mut dirs_done = 0u32;
        while let Some((data, path, parent_deleted)) = queue.pop_front() {
            if self.ctx.cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            dirs_done += 1;
            (self.ctx.progress)((dirs_done as f32 / (dirs_done as f32 + queue.len() as f32 + 1.0)).min(0.99));
            let mut entries = parse_dir(&data, &mut self.label);
            guess_first_letters(&mut entries);
            for e in entries {
                if e.is_dir {
                    if !c.valid(e.cluster) || !self.seen_dirs.insert(e.cluster) || self.seen_dirs.len() > 200_000 {
                        continue;
                    }
                    let sub = format!("{}\\{}", path, e.name);
                    if !e.deleted && !parent_deleted {
                        let d = c.read_clusters(&c.fat.chain(e.cluster, 1 << 14));
                        queue.push_back((d, sub, false));
                    } else if let Some((cl, d)) = read_deleted_dir(c, e.cluster) {
                        self.seen_dirs.insert(cl);
                        queue.push_back((d, sub, true));
                    }
                    continue;
                }
                if !self.collect {
                    continue;
                }
                let Some(fmt) = media_name(&e.name) else { continue };
                if !self.ctx.wants(fmt) || e.size == 0 {
                    continue;
                }
                let deleted = e.deleted || parent_deleted;
                let mut f = FoundFile {
                    id: 0,
                    name: e.name.clone(),
                    path: path.clone(),
                    volume: self.label.clone(),
                    format: fmt,
                    size: e.size as u64,
                    extents: Vec::new(),
                    origin: Origin::Fat,
                    state: if deleted { FileState::Deleted } else { FileState::Existing },
                    health: Health::Unknown,
                    note: String::new(),
                    modified: e.mtime,
                    created: e.ctime,
                    info: Default::default(),
                    fix_jpeg_eoi: false,
                };
                if !deleted {
                    let need = (e.size as u64).div_ceil(cs);
                    let chain = c.fat.chain(e.cluster, need + 1);
                    f.extents = merge_runs(chain.iter().map(|&cl| (Some(c.cluster_off(cl)), cs)), f.size);
                    f.health = Health::Good;
                } else {
                    recover_deleted(c, self.alloc, &e, fmt, &mut f);
                }
                self.files.push(f);
            }
        }
        Ok(())
    }
}

pub fn scan(ctx: &FsContext) -> Result<FsScan, String> {
    let (c, alloc) = open(ctx)?;
    let kind = c.boot.kind;
    let mut w = Walk { c: &c, alloc: &alloc, ctx, label: c.boot.label.clone(), seen_dirs: HashSet::new(), files: Vec::new(), collect: true };
    w.run(vec![(root_dir(&c), String::new(), false)])?;
    (ctx.progress)(1.0);
    let (label, files) = (w.label, w.files);
    Ok(FsScan { kind, label, files, alloc: Some(alloc), warnings: Vec::new() })
}

/// Recover files from directory clusters that are no longer reachable from the root
/// (their parent folder was overwritten). `offsets` are absolute offsets of sectors that
/// look like the start of a FAT directory.
pub fn scan_orphans(ctx: &FsContext, offsets: &[u64]) -> Vec<FoundFile> {
    let Ok((c, alloc)) = open(ctx) else { return Vec::new() };
    let mut w = Walk { c: &c, alloc: &alloc, ctx, label: c.boot.label.clone(), seen_dirs: HashSet::new(), files: Vec::new(), collect: false };
    if w.run(vec![(root_dir(&c), String::new(), false)]).is_err() {
        return Vec::new();
    }
    let heap = c.vol_off + c.data_start;
    let mut orphans = Vec::new();
    for &o in offsets {
        if o < heap || (o - heap) % c.boot.cluster != 0 {
            continue;
        }
        let cl = ((o - heap) / c.boot.cluster) as u32 + 2;
        if !c.valid(cl) || w.seen_dirs.contains(&cl) {
            continue;
        }
        orphans.push(cl);
    }
    // A folder that is referenced as a sub-folder of another orphan is not a top-level orphan.
    let mut start = Vec::new();
    w.collect = true;
    for cl in &orphans {
        if w.seen_dirs.contains(cl) {
            continue;
        }
        if let Some((cl2, d)) = read_deleted_dir(&c, *cl) {
            w.seen_dirs.insert(cl2);
            start.push((d, format!("\\Lost Folders\\Folder {}", cl2), true));
        }
    }
    let _ = w.run(start);
    w.files
}

/// Deleting a FAT file overwrites the first character of its 8.3 name. Rebuild it from
/// sibling files (IMG_0012.JPG next to _MG_0013.JPG) or common camera naming schemes.
fn guess_first_letters(entries: &mut [Entry]) {
    let known: Vec<String> = entries.iter().filter(|e| !e.lost_first).map(|e| e.name.to_uppercase()).collect();
    for e in entries.iter_mut().filter(|e| e.lost_first && e.name.len() > 4) {
        let up = e.name.to_uppercase();
        let tail = &up[1..4];
        let from_sibling = known.iter().find(|k| k.len() > 4 && &k[1..4] == tail && !k.starts_with('_')).and_then(|k| k.chars().next());
        let rest = &up[1..];
        let digits7 = rest.len() >= 7 && rest[..7].bytes().all(|b| b.is_ascii_digit());
        let guess = from_sibling.or(match () {
            _ if rest.starts_with("MG_") || rest.starts_with("MGP") => Some('I'),
            _ if rest.starts_with("SC") || rest.starts_with("JI_") => Some('D'),
            _ if rest.starts_with("VI_") || rest.starts_with("AH0") || rest.starts_with("OV_") => Some('M'),
            _ if rest.starts_with("OPR") || rest.starts_with("X0") || rest.starts_with("H0") => Some('G'),
            _ if rest.starts_with("ID_") => Some('V'),
            _ if rest.starts_with("ICT") || digits7 => Some('P'),
            _ if rest.starts_with("AM_") => Some('S'),
            _ if rest.starts_with("IMG") => Some('C'),
            _ if rest.starts_with("PIM") => Some('H'),
            _ => None,
        });
        if let Some(c) = guess {
            let first = if e.name[1..].chars().next().is_some_and(|c| c.is_lowercase()) { c.to_ascii_lowercase() } else { c };
            e.name.replace_range(..1, &first.to_string());
        }
    }
}

/// A deleted directory's clusters are assumed contiguous; read while entries keep looking valid.
fn read_deleted_dir(c: &Ctx, start: u32) -> Option<(u32, Vec<u8>)> {
    // A directory starts with "." and ".." entries.
    let is_dir = |b: &[u8]| &b[..2] == b". " && b[11] & 0x10 != 0 && &b[32..34] == b"..";
    let mut start = start;
    let mut first = c.read_clusters(&[start]);
    if !is_dir(&first) {
        if c.boot.kind != FsKind::Fat32 || start >> 16 != 0 {
            return None;
        }
        // FAT32 delete may have cleared the high word of the start cluster.
        let max_hi = ((c.boot.clusters + 2) >> 16) as u32;
        let lo = start;
        let found = (1..=max_hi).map(|hi| (hi << 16) | lo).find(|&cl| c.valid(cl) && c.fat.get(cl) == 0 && is_dir(&c.head(cl)));
        start = found?;
        first = c.read_clusters(&[start]);
    }
    let mut out = first;
    let mut cl = start + 1;
    while out.len() < (64 * c.boot.cluster) as usize && c.valid(cl) {
        if out.chunks_exact(32).any(|e| e[0] == 0) {
            break;
        }
        let next = c.read_clusters(&[cl]);
        let plausible = next.chunks_exact(32).take(4).all(|e| e[0] == 0 || e[0] == 0xE5 || e[11] == 0x0F || e[11] & 0xC0 == 0);
        if !plausible {
            break;
        }
        out.extend(next);
        cl += 1;
    }
    Some((start, out))
}

fn recover_deleted(c: &Ctx, alloc: &AllocMap, e: &Entry, fmt: Format, f: &mut FoundFile) {
    let cs = c.boot.cluster;
    let need = (e.size as u64).div_ceil(cs);
    let fam = fmt.family();
    let header_ok = |cl: u32| formats::identify(&c.head(cl)).is_some_and(|x| x.family() == fam);

    // Candidate start clusters: as recorded, plus FAT32 high-word reconstructions
    // (Windows clears the upper 16 bits of the start cluster on delete).
    let mut starts = vec![e.cluster];
    if c.boot.kind == FsKind::Fat32 && e.cluster >> 16 == 0 && !(c.valid(e.cluster) && header_ok(e.cluster)) {
        let lo = e.cluster & 0xFFFF;
        let max_hi = ((c.boot.clusters + 2) >> 16) as u32;
        for hi in 1..=max_hi {
            let cl = (hi << 16) | lo;
            if c.valid(cl) && c.fat.get(cl) == 0 && header_ok(cl) {
                starts.push(cl);
            }
        }
    }
    let mut best: Option<(Vec<Extent>, super::Verification, f64, u32)> = None;
    for &start in &starts {
        if !c.valid(start) {
            continue;
        }
        // Layout A: contiguous. Layout B: contiguous over currently-free clusters only.
        let contiguous: Vec<u32> = (0..need as u32).map(|i| start + i).filter(|&x| c.valid(x)).collect();
        let mut skip_alloc = Vec::new();
        let mut cl = start;
        while (skip_alloc.len() as u64) < need && c.valid(cl) {
            if c.fat.get(cl) == 0 || cl == start {
                skip_alloc.push(cl);
            }
            cl += 1;
        }
        let mut layouts = vec![contiguous];
        if skip_alloc != layouts[0] {
            layouts.push(skip_alloc);
        }
        for (li, l) in layouts.into_iter().enumerate() {
            let ext = merge_runs(l.iter().map(|&x| (Some(c.cluster_off(x)), cs)), e.size as u64);
            let overlap = alloc.overlap(&ext);
            let v = verify(c.dev, &ext, e.size as u64, fmt, false);
            let score = v.health as u32 * 4 + if overlap > 0.0 { 2 } else { 0 } + li as u32;
            if best.as_ref().is_none_or(|b| score < b.3) {
                best = Some((ext, v, overlap, score));
            }
        }
    }
    match best {
        Some((ext, v, overlap, _)) => {
            f.extents = ext;
            apply_verification(f, v, overlap);
        }
        None => {
            f.health = Health::Overwritten;
            f.note = "Start cluster invalid".into();
            f.extents = vec![Extent::at(c.vol_off, 0)];
        }
    }
    if starts.len() > 1 && f.extents.first().and_then(|x| x.offset) != Some(c.cluster_off(e.cluster)) {
        if f.note.is_empty() {
            f.note = "Start cluster reconstructed".into();
        }
    }
}
