//! exFAT (SDXC cards, large USB drives, camera media): walk live and deleted directory entry
//! sets. Deleted entries keep their stream extension (first cluster, length, contiguity
//! flag), so camera videos usually come back perfectly.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::Ordering;

use super::{apply_verification, media_name, merge_runs, verify, AllocMap, FsContext, FsKind, FsScan};
use crate::device::{read_tolerant, Dev};
use crate::model::{FileState, FoundFile, Health, Origin};
use crate::util::{dos_to_unix, le16, le32, le64};

pub struct Boot {
    pub bps: u64,
    pub cluster: u64,
    pub vol_len_sectors: u64,
    pub fat_off: u64,
    pub heap_off: u64,
    pub clusters: u64,
    pub root: u32,
}

impl Boot {
    pub fn parse(b: &[u8]) -> Option<Boot> {
        if b.len() < 512 || &b[3..11] != b"EXFAT   " {
            return None;
        }
        let bps_shift = b[0x6C] as u32;
        let spc_shift = b[0x6D] as u32;
        if !(9..=12).contains(&bps_shift) || spc_shift > 25 - bps_shift {
            return None;
        }
        let bps = 1u64 << bps_shift;
        Some(Boot {
            bps,
            cluster: bps << spc_shift,
            vol_len_sectors: le64(b, 0x48),
            fat_off: le32(b, 0x50) as u64 * bps,
            heap_off: le32(b, 0x58) as u64 * bps,
            clusters: le32(b, 0x5C) as u64,
            root: le32(b, 0x60),
        })
    }
    pub fn total_bytes(&self) -> u64 {
        self.vol_len_sectors * self.bps
    }
}

struct Ctx<'a> {
    dev: &'a Dev,
    vol_off: u64,
    boot: Boot,
    fat: Vec<u32>,
}

impl Ctx<'_> {
    fn off(&self, c: u32) -> u64 {
        self.vol_off + self.boot.heap_off + (c as u64 - 2) * self.boot.cluster
    }
    fn valid(&self, c: u32) -> bool {
        c >= 2 && (c as u64) < self.boot.clusters + 2
    }
    fn chain(&self, start: u32, max: u64) -> Vec<u32> {
        let mut out = Vec::new();
        let mut c = start;
        while self.valid(c) && (out.len() as u64) < max {
            out.push(c);
            let n = self.fat.get(c as usize).copied().unwrap_or(0);
            if n >= 0xFFFF_FFF7 || n < 2 || n == c {
                break;
            }
            c = n;
        }
        out
    }
    /// Cluster list for a stream: contiguous when flagged NoFatChain, else via the FAT.
    fn clusters_for(&self, first: u32, len: u64, contiguous: bool) -> Vec<u32> {
        let need = len.div_ceil(self.boot.cluster).max(1);
        if contiguous {
            return (0..need as u32).map(|i| first + i).filter(|&c| self.valid(c)).collect();
        }
        let ch = self.chain(first, need);
        if ch.len() as u64 == need { ch } else { (0..need as u32).map(|i| first + i).filter(|&c| self.valid(c)).collect() }
    }
    fn read(&self, cl: &[u32], len: u64) -> Vec<u8> {
        let cs = self.boot.cluster as usize;
        let mut out = vec![0u8; cl.len() * cs];
        for (i, &c) in cl.iter().enumerate() {
            read_tolerant(&**self.dev, self.off(c), &mut out[i * cs..(i + 1) * cs]);
        }
        out.truncate((len as usize).min(out.len()));
        out
    }
}

struct Entry {
    name: String,
    deleted: bool,
    is_dir: bool,
    first: u32,
    len: u64,
    contiguous: bool,
    ctime: Option<i64>,
    mtime: Option<i64>,
}

fn ts(v: u32) -> Option<i64> {
    dos_to_unix((v >> 16) as u16, v as u16)
}

fn parse_dir(d: &[u8], label: &mut String, bitmap: &mut Option<(u32, u64)>) -> Vec<Entry> {
    let ents: Vec<&[u8]> = d.chunks_exact(32).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < ents.len() {
        let e = ents[i];
        let t = e[0];
        if t == 0 {
            break;
        }
        match t {
            0x83 => {
                let n = (e[1] as usize).min(11);
                let u: Vec<u16> = e[2..2 + 2 * n].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                *label = String::from_utf16_lossy(&u);
            }
            0x81 => *bitmap = Some((le32(e, 20), le64(e, 24))),
            0x85 | 0x05 => {
                let deleted = t == 0x05;
                let sec = e[1] as usize;
                if (2..=18).contains(&sec) && i + sec < ents.len() {
                    let s = ents[i + 1];
                    let stream_ok = if deleted { s[0] == 0x40 || s[0] == 0xC0 } else { s[0] == 0xC0 };
                    if stream_ok {
                        let name_len = s[3] as usize;
                        let mut units = Vec::with_capacity(name_len);
                        for k in 2..=sec {
                            let n = ents[i + k];
                            if n[0] & 0x7F != 0x41 {
                                break;
                            }
                            for c in n[2..32].chunks_exact(2) {
                                units.push(u16::from_le_bytes([c[0], c[1]]));
                            }
                        }
                        units.truncate(name_len);
                        let name = String::from_utf16_lossy(&units);
                        if !name.is_empty() {
                            out.push(Entry {
                                name,
                                deleted,
                                is_dir: le16(e, 4) & 0x10 != 0,
                                first: le32(s, 0x14),
                                len: le64(s, 0x18),
                                contiguous: s[1] & 0x02 != 0,
                                ctime: ts(le32(e, 8)),
                                mtime: ts(le32(e, 0x0C)),
                            });
                        }
                        i += 1 + sec;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

pub fn scan(ctx: &FsContext) -> Result<FsScan, String> {
    let dev = &ctx.dev;
    let mut bs = vec![0u8; 512];
    read_tolerant(&**dev, ctx.vol_off, &mut bs);
    let boot = Boot::parse(&bs).ok_or("Not an exFAT boot sector")?;
    let mut raw = vec![0u8; ((boot.clusters + 2) * 4) as usize];
    read_tolerant(&**dev, ctx.vol_off + boot.fat_off, &mut raw);
    let fat: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let c = Ctx { dev, vol_off: ctx.vol_off, boot, fat };
    let cs = c.boot.cluster;

    let root = c.read(&c.chain(c.boot.root, 1 << 16), u64::MAX);
    let mut label = String::new();
    let mut bitmap = None;
    let mut queue: VecDeque<(Vec<u8>, String, bool)> = VecDeque::new();
    queue.push_back((root, String::new(), false));
    let mut entries: Vec<(Entry, String, bool)> = Vec::new();
    let mut seen = HashSet::new();
    let mut done = 0u32;
    while let Some((data, path, parent_deleted)) = queue.pop_front() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        done += 1;
        (ctx.progress)((done as f32 / (done as f32 + queue.len() as f32 + 1.0)).min(0.5));
        for e in parse_dir(&data, &mut label, &mut bitmap) {
            if e.is_dir {
                if c.valid(e.first) && e.len > 0 && e.len < 256 << 20 && seen.insert(e.first) {
                    let cl = c.clusters_for(e.first, e.len, e.contiguous || e.deleted || parent_deleted);
                    let d = c.read(&cl, e.len);
                    queue.push_back((d, format!("{}\\{}", path, e.name), e.deleted || parent_deleted));
                }
                continue;
            }
            entries.push((e, path.clone(), parent_deleted));
        }
    }

    // Allocation bitmap.
    let mut alloc = AllocMap::new(ctx.vol_off, ctx.vol_off + ctx.vol_len, ctx.vol_off + c.boot.heap_off, 2, cs, c.boot.clusters);
    if let Some((first, len)) = bitmap {
        let bytes = c.read(&c.chain(first, len.div_ceil(cs)), len);
        for (i, w) in bytes.chunks(8).enumerate() {
            let mut a = [0u8; 8];
            a[..w.len()].copy_from_slice(w);
            if i < alloc.bits.len() {
                alloc.bits[i] = u64::from_le_bytes(a);
            }
        }
    }

    let mut files = Vec::new();
    let total = entries.len().max(1);
    for (k, (e, path, parent_deleted)) in entries.into_iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let Some(fmt) = media_name(&e.name) else { continue };
        if !ctx.wants(fmt) || e.len == 0 || !c.valid(e.first) {
            continue;
        }
        let deleted = e.deleted || parent_deleted;
        let cl = c.clusters_for(e.first, e.len, e.contiguous || deleted && !fat_chain_intact(&c, &e));
        let extents = merge_runs(cl.iter().map(|&x| (Some(c.off(x)), cs)), e.len);
        let mut f = FoundFile {
            id: 0,
            name: e.name,
            path,
            volume: label.clone(),
            format: fmt,
            size: e.len,
            extents,
            origin: Origin::ExFat,
            state: if deleted { FileState::Deleted } else { FileState::Existing },
            health: Health::Good,
            note: String::new(),
            modified: e.mtime,
            created: e.ctime,
            info: Default::default(),
            fix_jpeg_eoi: false,
        };
        if deleted {
            let overlap = alloc.overlap(&f.extents);
            let v = verify(dev, &f.extents, f.size, fmt, false);
            apply_verification(&mut f, v, overlap);
        }
        files.push(f);
        if k % 32 == 0 {
            (ctx.progress)(0.5 + 0.5 * k as f32 / total as f32);
        }
    }
    (ctx.progress)(1.0);
    Ok(FsScan { kind: FsKind::ExFat, label, files, alloc: Some(alloc), warnings: Vec::new() })
}

/// After deletion the FAT chain may survive; trust it only if it has exactly the right length.
fn fat_chain_intact(c: &Ctx, e: &Entry) -> bool {
    let need = e.len.div_ceil(c.boot.cluster);
    let ch = c.chain(e.first, need + 1);
    ch.len() as u64 == need && need > 1
}
