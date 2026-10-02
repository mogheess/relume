//! NTFS: parse every MFT record (in-use and deleted), rebuild folder paths, follow data runs
//! (so fragmented deleted files come back whole), restore Recycle Bin names from $I files,
//! and check each deleted file's clusters against $Bitmap and its actual content.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use super::{apply_verification, format_for_name, media_name, merge_runs, verify, AllocMap, FsContext, FsKind, FsScan};
use crate::device::{read_tolerant, ExtentDevice};
use crate::model::{Extent, FileState, FoundFile, Health, Origin};
use crate::util::{filetime_to_unix, le16, le32, le64, utf16le_string};

pub struct Boot {
    pub bps: u64,
    pub cluster: u64,
    pub total_sectors: u64,
    pub mft_lcn: u64,
    pub mftmirr_lcn: u64,
    pub rec_size: u64,
}

impl Boot {
    pub fn parse(b: &[u8]) -> Option<Boot> {
        if b.len() < 512 || &b[3..11] != b"NTFS    " {
            return None;
        }
        let bps = le16(b, 0x0B) as u64;
        if !matches!(bps, 512 | 1024 | 2048 | 4096) {
            return None;
        }
        let spc_raw = b[0x0D];
        let spc = if spc_raw <= 0x80 { spc_raw as u64 } else { 1u64 << (256 - spc_raw as u32) };
        if spc == 0 || !spc.is_power_of_two() {
            return None;
        }
        let cluster = bps * spc;
        let cpr = b[0x40] as i8;
        let rec_size = if cpr > 0 { cpr as u64 * cluster } else { 1u64 << (-(cpr as i32)) };
        if !(256..=65536).contains(&rec_size) {
            return None;
        }
        Some(Boot {
            bps,
            cluster,
            total_sectors: le64(b, 0x28),
            mft_lcn: le64(b, 0x30),
            mftmirr_lcn: le64(b, 0x38),
            rec_size,
        })
    }
    pub fn total_bytes(&self) -> u64 {
        (self.total_sectors + 1) * self.bps
    }
}

/// Apply the update-sequence fixups of a multi-sector record. Returns false if torn.
fn fixup(rec: &mut [u8]) -> bool {
    let usa_off = le16(rec, 4) as usize;
    let usa_cnt = le16(rec, 6) as usize;
    if usa_cnt < 2 || usa_off + usa_cnt * 2 > rec.len() {
        return false;
    }
    let usn = [rec[usa_off], rec[usa_off + 1]];
    let mut ok = true;
    for i in 1..usa_cnt {
        let end = i * 512;
        if end > rec.len() {
            break;
        }
        if rec[end - 2..end] != usn {
            ok = false;
        }
        rec[end - 2] = rec[usa_off + 2 * i];
        rec[end - 1] = rec[usa_off + 2 * i + 1];
    }
    ok
}

/// Decode a data-run list into (lcn or None for sparse, cluster count).
fn decode_runs(b: &[u8], total_clusters: u64) -> Option<Vec<(Option<u64>, u64)>> {
    let mut out = Vec::new();
    let mut p = 0;
    let mut lcn: i64 = 0;
    while p < b.len() {
        let h = b[p];
        if h == 0 {
            break;
        }
        let ls = (h & 0xF) as usize;
        let os = (h >> 4) as usize;
        p += 1;
        if ls == 0 || ls > 8 || os > 8 || p + ls + os > b.len() {
            return None;
        }
        let mut len = 0u64;
        for i in 0..ls {
            len |= (b[p + i] as u64) << (8 * i);
        }
        p += ls;
        if os == 0 {
            out.push((None, len));
        } else {
            let mut d: i64 = 0;
            for i in 0..os {
                d |= (b[p + i] as i64) << (8 * i);
            }
            if b[p + os - 1] & 0x80 != 0 && os < 8 {
                d -= 1i64 << (8 * os);
            }
            p += os;
            lcn += d;
            if lcn < 0 || lcn as u64 + len > total_clusters + 16 {
                return None;
            }
            out.push((Some(lcn as u64), len));
        }
        if out.len() > 1_000_000 {
            return None;
        }
    }
    Some(out)
}

#[derive(Default)]
struct DataAttr {
    start_vcn: u64,
    runs: Vec<(Option<u64>, u64)>,
    real_size: Option<u64>,
    flags: u16,
    resident: Option<Vec<u8>>,
}

#[derive(Default)]
struct Rec {
    in_use: bool,
    is_dir: bool,
    seq: u16,
    base: u64,
    is_ext: bool,
    attr_list: Vec<(u32, u64, u64)>, // (type, start vcn, record)
    attr_list_runs: Option<(Vec<(Option<u64>, u64)>, u64)>, // non-resident list: (runs, size)
    name: Option<(u8, String, u64)>, // (namespace, name, parent ref)
    fn_size: u64,
    si_ctime: Option<i64>,
    si_mtime: Option<i64>,
    si_attrs: u32,
    data: Vec<DataAttr>,
    vol_name: Option<String>,
}

fn parse_record(rec: &mut [u8], total_clusters: u64) -> Option<Rec> {
    if &rec[..4] != b"FILE" {
        return None;
    }
    fixup(rec);
    let flags = le16(rec, 0x16);
    let used = (le32(rec, 0x18) as usize).min(rec.len());
    let base_raw = le64(rec, 0x20);
    let mut r = Rec {
        in_use: flags & 1 != 0,
        is_dir: flags & 2 != 0,
        seq: le16(rec, 0x10),
        base: base_raw & 0xFFFF_FFFF_FFFF,
        is_ext: base_raw != 0,
        ..Default::default()
    };
    let mut p = le16(rec, 0x14) as usize;
    let rank = |ns: u8| match ns {
        1 | 3 => 3,
        0 => 2,
        _ => 1,
    };
    while p + 16 <= used {
        let typ = le32(rec, p);
        if typ == 0xFFFF_FFFF {
            break;
        }
        let len = le32(rec, p + 4) as usize;
        if len < 16 || p + len > used {
            break;
        }
        let a = &rec[p..p + len];
        let nonres = a[8] != 0;
        let name_len = a[9];
        let aflags = le16(a, 0x0C);
        let value = || -> Option<&[u8]> {
            if nonres || a.len() < 0x18 {
                return None;
            }
            let vl = le32(a, 0x10) as usize;
            let vo = le16(a, 0x14) as usize;
            a.get(vo..vo + vl)
        };
        match typ {
            0x10 => {
                if let Some(v) = value().filter(|v| v.len() >= 0x24) {
                    r.si_ctime = filetime_to_unix(le64(v, 0));
                    r.si_mtime = filetime_to_unix(le64(v, 8));
                    r.si_attrs = le32(v, 0x20);
                }
            }
            0x30 => {
                if let Some(v) = value().filter(|v| v.len() >= 0x42) {
                    let n = v[0x40] as usize;
                    let ns = v[0x41];
                    if v.len() >= 0x42 + 2 * n {
                        let name = utf16le_string(&v[0x42..0x42 + 2 * n]);
                        let better = match &r.name {
                            None => true,
                            Some((old, _, _)) => rank(ns) > rank(*old),
                        };
                        if better {
                            r.name = Some((ns, name, le64(v, 0)));
                            r.fn_size = le64(v, 0x30);
                        }
                    }
                }
            }
            0x20 => {
                if let Some(v) = value() {
                    r.attr_list = parse_attr_list(v);
                } else if nonres && a.len() >= 0x40 {
                    let ro = le16(a, 0x20) as usize;
                    if let Some(runs) = a.get(ro..).and_then(|b| decode_runs(b, total_clusters)) {
                        r.attr_list_runs = Some((runs, le64(a, 0x30)));
                    }
                }
            }
            0x60 => {
                if let Some(v) = value() {
                    r.vol_name = Some(utf16le_string(v));
                }
            }
            0x80 if name_len == 0 => {
                if nonres {
                    if a.len() >= 0x40 {
                        let start_vcn = le64(a, 0x10);
                        let ro = le16(a, 0x20) as usize;
                        if let Some(runs) = a.get(ro..).and_then(|b| decode_runs(b, total_clusters)) {
                            r.data.push(DataAttr {
                                start_vcn,
                                runs,
                                real_size: (start_vcn == 0).then(|| le64(a, 0x30)),
                                flags: aflags,
                                resident: None,
                            });
                        }
                    }
                } else if let Some(v) = value() {
                    r.data.push(DataAttr { real_size: Some(v.len() as u64), resident: Some(v.to_vec()), flags: aflags, ..Default::default() });
                }
            }
            _ => {}
        }
        p += len;
    }
    Some(r)
}

fn parse_attr_list(v: &[u8]) -> Vec<(u32, u64, u64)> {
    let mut out = Vec::new();
    let mut q = 0;
    while q + 0x1A <= v.len() {
        let l = le16(v, q + 4) as usize;
        if l < 0x1A {
            break;
        }
        out.push((le32(v, q), le64(v, q + 8), le64(v, q + 0x10) & 0xFFFF_FFFF_FFFF));
        q += l;
    }
    out
}

struct Dir {
    name: String,
    parent: u64,
    parent_seq: u16,
    seq: u16,
}

struct Cand {
    rec: u64,
    in_use: bool,
    name: String,
    parent: u64,
    parent_seq: u16,
    fn_size: u64,
    data: Vec<DataAttr>,
    ctime: Option<i64>,
    mtime: Option<i64>,
    attrs: u32,
}

pub fn scan(ctx: &FsContext) -> Result<FsScan, String> {
    let dev = &ctx.dev;
    let mut bs = vec![0u8; 512];
    read_tolerant(&**dev, ctx.vol_off, &mut bs);
    let boot = Boot::parse(&bs).ok_or("Not an NTFS boot sector")?;
    let cluster = boot.cluster;
    let total_clusters = boot.total_bytes() / cluster;
    let rs = boot.rec_size as usize;
    let vol = |lcn: u64| ctx.vol_off + lcn * cluster;
    let mut warnings = Vec::new();

    // $MFT's own record gives the MFT layout (try the mirror if damaged).
    let mut mft_runs = None;
    let mut mft_size = 0;
    for lcn in [boot.mft_lcn, boot.mftmirr_lcn] {
        let mut rec = vec![0u8; rs];
        read_tolerant(&**dev, vol(lcn), &mut rec);
        let Some(r) = parse_record(&mut rec, total_clusters) else {
            warnings.push("Primary MFT record damaged, using mirror".to_string());
            continue;
        };
        let Some(d) = r.data.iter().find(|d| d.start_vcn == 0 && d.resident.is_none()) else { continue };
        mft_size = d.real_size.unwrap_or(0);
        let mut segs: Vec<(u64, Vec<(Option<u64>, u64)>)> = vec![(0, d.runs.clone())];
        // A heavily fragmented MFT keeps further runs in extension records (attribute list).
        let first_ext: Vec<Extent> = d.runs.iter().map(|(l, n)| Extent { offset: l.map(vol), len: n * cluster }).collect();
        let first_dev = ExtentDevice::new(dev.clone(), first_ext, u64::MAX >> 1);
        let mut list = r.attr_list.clone();
        if list.is_empty() {
            if let Some((runs, len)) = &r.attr_list_runs {
                let ext: Vec<Extent> = runs.iter().map(|(l, n)| Extent { offset: l.map(vol), len: n * cluster }).collect();
                let mut buf = vec![0u8; (*len).min(16 << 20) as usize];
                read_tolerant(&ExtentDevice::new(dev.clone(), ext, *len), 0, &mut buf);
                list = parse_attr_list(&buf);
            }
        }
        for &(t, vcn, recno) in &list {
            if t != 0x80 || recno == 0 || vcn == 0 {
                continue;
            }
            let mut e = vec![0u8; rs];
            read_tolerant(&first_dev, recno * boot.rec_size, &mut e);
            if let Some(er) = parse_record(&mut e, total_clusters) {
                for dd in er.data.into_iter().filter(|d| d.start_vcn > 0) {
                    segs.push((dd.start_vcn, dd.runs));
                }
            }
        }
        segs.sort_by_key(|s| s.0);
        segs.dedup_by_key(|s| s.0);
        mft_runs = Some(segs.into_iter().flat_map(|s| s.1).collect::<Vec<_>>());
        break;
    }
    let mft_runs = mft_runs.ok_or("MFT not readable")?;
    let mft_extents: Vec<Extent> = mft_runs.iter().map(|(l, n)| Extent { offset: l.map(vol), len: n * cluster }).collect();
    let mft_len = mft_size.min(mft_extents.iter().map(|e| e.len).sum());
    let mft_dev = ExtentDevice::new(dev.clone(), mft_extents, mft_len);
    let total_recs = mft_len / boot.rec_size;

    let mut dirs: HashMap<u64, Dir> = HashMap::new();
    let mut cands: Vec<Cand> = Vec::new();
    let mut ext_data: HashMap<u64, Vec<DataAttr>> = HashMap::new();
    let mut recycle_info: RecycleMap = HashMap::new();
    let mut bitmap_attr: Option<DataAttr> = None;
    let mut label = String::new();

    let batch = (4 << 20) / rs;
    let mut buf = vec![0u8; batch * rs];
    let mut recno = 0u64;
    while recno < total_recs {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let n = ((total_recs - recno) as usize).min(batch);
        let bytes = &mut buf[..n * rs];
        read_tolerant(&mft_dev, recno * boot.rec_size, bytes);
        for i in 0..n {
            let rn = recno + i as u64;
            let rec = &mut bytes[i * rs..(i + 1) * rs];
            let Some(r) = parse_record(rec, total_clusters) else { continue };
            if rn == 3 {
                if let Some(v) = &r.vol_name {
                    label = v.clone();
                }
            }
            if rn == 6 {
                bitmap_attr = r.data.into_iter().find(|d| d.start_vcn == 0);
                continue;
            }
            if r.is_ext {
                // Extension record: belongs to the base record's attribute list.
                if r.base != 0 && !r.data.is_empty() {
                    ext_data.entry(r.base).or_default().extend(r.data);
                }
                continue;
            }
            let Some((_, name, parent_ref)) = r.name else { continue };
            let parent = parent_ref & 0xFFFF_FFFF_FFFF;
            let parent_seq = (parent_ref >> 48) as u16;
            if r.is_dir {
                dirs.insert(rn, Dir { name, parent, parent_seq, seq: r.seq });
                continue;
            }
            let upper = name.to_ascii_uppercase();
            if upper.starts_with("$I") {
                if let Some(v) = r.data.iter().find_map(|d| d.resident.as_ref()) {
                    if let Some(info) = parse_recycle_info(v) {
                        recycle_info.insert((parent, upper[2..].to_string()), info);
                    }
                }
                continue;
            }
            let Some(fmt) = media_name(&name) else { continue };
            if !ctx.wants(fmt) || rn < 24 {
                continue;
            }
            cands.push(Cand {
                rec: rn,
                in_use: r.in_use,
                name,
                parent,
                parent_seq,
                fn_size: r.fn_size,
                data: r.data,
                ctime: r.si_ctime,
                mtime: r.si_mtime,
                attrs: r.si_attrs,
            });
        }
        recno += n as u64;
        (ctx.progress)(recno as f32 / total_recs.max(1) as f32);
    }

    // Allocation bitmap.
    let alloc = bitmap_attr.and_then(|b| {
        let ext: Vec<Extent> = b.runs.iter().map(|(l, n)| Extent { offset: l.map(vol), len: n * cluster }).collect();
        let len = b.real_size.unwrap_or(0).min(total_clusters.div_ceil(8) + 8);
        let bd = ExtentDevice::new(dev.clone(), ext, len);
        let mut bytes = vec![0u8; len as usize];
        read_tolerant(&bd, 0, &mut bytes);
        let mut m = AllocMap::new(ctx.vol_off, ctx.vol_off + ctx.vol_len, ctx.vol_off, 0, cluster, total_clusters);
        for (i, w) in bytes.chunks(8).enumerate() {
            let mut a = [0u8; 8];
            a[..w.len()].copy_from_slice(w);
            if i < m.bits.len() {
                m.bits[i] = u64::from_le_bytes(a);
            }
        }
        Some(m)
    });
    if alloc.is_none() {
        warnings.push("$Bitmap unreadable: overwrite detection limited".into());
    }

    // Folder paths.
    let mut memo: HashMap<u64, (String, bool)> = HashMap::new();
    let mut files = Vec::new();
    let total = cands.len().max(1);
    for (ci, c) in cands.into_iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let (mut path, in_bin) = dir_path(c.parent, c.parent_seq, &dirs, &recycle_info, &mut memo);
        let mut name = c.name.clone();
        let mut state = if c.in_use { FileState::Existing } else { FileState::Deleted };
        let mut mtime = c.mtime;
        let upper = name.to_ascii_uppercase();
        if in_bin {
            if upper.starts_with("$R") {
                if let Some((orig, deleted_at)) = recycle_info.get(&(c.parent, upper[2..].to_string())) {
                    let orig = strip_drive(orig);
                    let (dir, fname) = orig.rsplit_once('\\').unwrap_or(("", orig));
                    name = fname.to_string();
                    path = dir.to_string();
                    if mtime.is_none() {
                        mtime = *deleted_at;
                    }
                }
            }
            state = FileState::RecycleBin;
        }
        // Data runs, merged with extension records, ordered by VCN.
        let mut segs: Vec<DataAttr> = c.data;
        if let Some(more) = ext_data.remove(&c.rec) {
            segs.extend(more);
        }
        segs.sort_by_key(|d| d.start_vcn);
        segs.dedup_by_key(|d| d.start_vcn);
        // Large fragmented files keep their whole $DATA (incl. the VCN-0 segment that holds
        // the real size) in extension records, so size is known only after merging.
        let size = segs.iter().find(|d| d.start_vcn == 0).and_then(|d| d.real_size).unwrap_or(c.fn_size);
        let flags = segs.first().map(|d| d.flags).unwrap_or(0);
        let resident = segs.iter().any(|d| d.resident.is_some());
        if resident || size == 0 || segs.is_empty() {
            continue; // tiny files stored inside the MFT record: not meaningful media
        }
        let runs = segs.iter().flat_map(|d| d.runs.iter().map(|(l, n)| (l.map(vol), n * cluster)));
        let extents = merge_runs(runs, size);
        let mut f = FoundFile {
            id: 0,
            name,
            path,
            volume: label.clone(),
            format: format_for_name(&c.name).unwrap(),
            size,
            extents,
            origin: Origin::Ntfs,
            state,
            health: Health::Unknown,
            note: String::new(),
            modified: mtime,
            created: c.ctime,
            info: Default::default(),
            fix_jpeg_eoi: false,
        };
        if flags & 0x0001 != 0 {
            f.health = Health::Damaged;
            f.note = "NTFS-compressed: recovered as stored".into();
        } else if flags & 0x4000 != 0 || c.attrs & 0x4000 != 0 {
            f.health = Health::Damaged;
            f.note = "EFS-encrypted: content unreadable without the user's key".into();
        } else if state == FileState::Existing {
            f.health = Health::Good;
        } else {
            let overlap = alloc.as_ref().map(|m| m.overlap(&f.extents)).unwrap_or(0.0);
            let v = verify(dev, &f.extents, f.size, f.format, false);
            apply_verification(&mut f, v, if state == FileState::RecycleBin && c.in_use { 0.0 } else { overlap });
        }
        files.push(f);
        if ci % 64 == 0 {
            (ctx.progress)(ci as f32 / total as f32);
        }
    }
    Ok(FsScan { kind: FsKind::Ntfs, label, files, alloc, warnings })
}

fn strip_drive(p: &str) -> &str {
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' { &p[2..] } else { p }
}

type RecycleMap = HashMap<(u64, String), (String, Option<i64>)>;

/// Build "\\folder\\sub" for a directory record. Folders deleted to the Recycle Bin
/// ($Rxxxxxx) are mapped back to their original location. Returns (path, inside recycle bin).
fn dir_path(rec: u64, want_seq: u16, dirs: &HashMap<u64, Dir>, bin: &RecycleMap, memo: &mut HashMap<u64, (String, bool)>) -> (String, bool) {
    if rec == 5 {
        return (String::new(), false);
    }
    if let Some(p) = memo.get(&rec) {
        return p.clone();
    }
    let mut chain = Vec::new();
    let mut cur = rec;
    let mut seq = want_seq;
    let mut prefix = (String::new(), false);
    for _ in 0..64 {
        if cur == 5 {
            break;
        }
        if let Some(p) = memo.get(&cur) {
            prefix = p.clone();
            break;
        }
        match dirs.get(&cur) {
            // Deleting a record bumps its sequence number, so a deleted parent is seq-1.
            Some(d) if d.seq == seq || d.seq.wrapping_sub(1) == seq || seq == 0 => {
                let upper = d.name.to_ascii_uppercase();
                if upper.starts_with("$R") {
                    if let Some((orig, _)) = bin.get(&(d.parent, upper[2..].to_string())) {
                        prefix = (strip_drive(orig).to_string(), true);
                        chain.push((cur, None));
                        break;
                    }
                }
                if upper == "$RECYCLE.BIN" {
                    prefix.1 = true;
                }
                chain.push((cur, Some(d.name.clone())));
                seq = d.parent_seq;
                cur = d.parent;
            }
            _ => {
                prefix = ("\\$Orphan Files".to_string(), false);
                break;
            }
        }
    }
    let in_bin = prefix.1 || chain.iter().any(|(_, n)| n.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("$Recycle.Bin")));
    let mut path = prefix.0;
    for (r, name) in chain.into_iter().rev() {
        if let Some(name) = name {
            path.push('\\');
            path.push_str(&name);
        }
        memo.insert(r, (path.clone(), in_bin));
    }
    memo.insert(rec, (path.clone(), in_bin));
    (path, in_bin)
}

/// Recycle Bin $I file: original path + deletion time.
fn parse_recycle_info(d: &[u8]) -> Option<(String, Option<i64>)> {
    if d.len() < 24 {
        return None;
    }
    let ver = le64(d, 0);
    let deleted = filetime_to_unix(le64(d, 16));
    let path = match ver {
        1 if d.len() >= 24 + 520 => utf16le_string(&d[24..24 + 520]),
        2 if d.len() >= 28 => {
            let n = le32(d, 24) as usize;
            utf16le_string(d.get(28..28 + 2 * n)?)
        }
        _ => return None,
    };
    (!path.is_empty()).then_some((path, deleted))
}
