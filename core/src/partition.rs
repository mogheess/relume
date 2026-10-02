//! Disk layout: MBR (incl. extended/logical), GPT, or a bare volume (superfloppy).

use crate::device::{read_full, Dev};
use crate::fs::{self, FsKind};
use crate::util::{le16, le32, le64, utf16le_string};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VolumeRegion {
    pub offset: u64,
    pub size: u64,
    pub fs: Option<FsKind>,
    pub name: String,
    /// Found by searching for boot sectors, not listed in the partition table.
    pub lost: bool,
}

impl VolumeRegion {
    pub fn describe(&self) -> String {
        let fs = self.fs.map(|f| f.label()).unwrap_or("Unknown");
        let size = crate::util::format_size(self.size);
        let mut s = if self.name.is_empty() { format!("{} volume", fs) } else { format!("{} ({})", self.name, fs) };
        s.push_str(&format!(" · {}", size));
        if self.lost {
            s.push_str(" · lost partition");
        }
        s
    }
}

fn sector(dev: &Dev, off: u64, len: usize) -> Vec<u8> {
    let mut b = vec![0u8; len];
    let n = read_full(&**dev, off, &mut b).unwrap_or(0);
    b.truncate(n);
    b
}

/// Detect the file system at `off` and build a region for it.
pub fn region_at(dev: &Dev, off: u64, fallback_size: u64, name: &str, lost: bool) -> VolumeRegion {
    let b = sector(dev, off, 512);
    let fs = fs::detect(&b);
    let size = fs.and_then(|_| fs::volume_size(&b)).filter(|s| *s > 0).unwrap_or(fallback_size);
    let size = size.min(dev.len().saturating_sub(off));
    VolumeRegion { offset: off, size, fs, name: name.to_string(), lost }
}

/// Enumerate the volumes on a device (disk, partition, or image file).
pub fn layout(dev: &Dev) -> Vec<VolumeRegion> {
    let s0 = sector(dev, 0, 512);
    if s0.len() < 512 {
        return Vec::new();
    }
    // A volume boot sector at LBA 0: the device is a single volume.
    if fs::detect(&s0).is_some() {
        return vec![region_at(dev, 0, dev.len(), "", false)];
    }
    if s0[510] != 0x55 || s0[511] != 0xAA {
        return Vec::new();
    }
    let entries: Vec<(u8, u64, u64)> = (0..4)
        .map(|i| {
            let e = &s0[0x1BE + i * 16..0x1BE + (i + 1) * 16];
            (e[4], le32(e, 8) as u64, le32(e, 12) as u64)
        })
        .filter(|e| e.0 != 0 && e.2 != 0)
        .collect();
    if entries.iter().any(|e| e.0 == 0xEE) {
        for ss in [512u64, 4096] {
            if let Some(v) = gpt(dev, ss) {
                return v;
            }
        }
    }
    // MBR LBAs are in logical sectors: 512 normally, 4096 on 4Kn disks and many USB
    // enclosures. Use the device's sector size, but trust whichever size finds file systems.
    let mut sizes = vec![dev.sector_size().max(512) as u64, 512, 4096];
    sizes.dedup();
    let mut best: Option<Vec<VolumeRegion>> = None;
    for ss in sizes {
        let v = mbr(dev, &entries, ss);
        if v.iter().any(|r| r.fs.is_some()) {
            return v;
        }
        best.get_or_insert(v);
    }
    best.unwrap_or_default()
}

fn mbr(dev: &Dev, entries: &[(u8, u64, u64)], ss: u64) -> Vec<VolumeRegion> {
    let mut out = Vec::new();
    let mut n = 1;
    for &(typ, lba, count) in entries {
        if matches!(typ, 0x05 | 0x0F | 0x85) {
            // Extended partition: chain of EBRs.
            let ext_start = lba * ss;
            let mut ebr = ext_start;
            for _ in 0..128 {
                let b = sector(dev, ebr, 512);
                if b.len() < 512 || b[510] != 0x55 || b[511] != 0xAA {
                    break;
                }
                let e0 = &b[0x1BE..0x1CE];
                let e1 = &b[0x1CE..0x1DE];
                if e0[4] != 0 && le32(e0, 12) != 0 {
                    let off = ebr + le32(e0, 8) as u64 * ss;
                    out.push(region_at(dev, off, le32(e0, 12) as u64 * ss, &format!("Partition {}", n), false));
                    n += 1;
                }
                if e1[4] == 0 || le32(e1, 8) == 0 {
                    break;
                }
                ebr = ext_start + le32(e1, 8) as u64 * ss;
            }
        } else if lba * ss < dev.len() {
            out.push(region_at(dev, lba * ss, count * ss, &format!("Partition {}", n), false));
            n += 1;
        }
    }
    out
}

fn gpt(dev: &Dev, ss: u64) -> Option<Vec<VolumeRegion>> {
    let h = sector(dev, ss, 512);
    if h.len() < 92 || &h[..8] != b"EFI PART" {
        return None;
    }
    let lba = le64(&h, 0x48);
    let count = le32(&h, 0x50).min(1024) as u64;
    let esz = le32(&h, 0x54) as u64;
    if esz < 128 {
        return None;
    }
    let tab = sector(dev, lba * ss, (count * esz) as usize);
    let mut out = Vec::new();
    for i in 0..count {
        let o = (i * esz) as usize;
        if o + 128 > tab.len() {
            break;
        }
        let e = &tab[o..o + 128];
        if e[..16].iter().all(|&b| b == 0) {
            continue;
        }
        let first = le64(e, 32);
        let last = le64(e, 40);
        if last < first {
            continue;
        }
        let mut name = utf16le_string(&e[56..128]);
        if name.is_empty() {
            name = format!("Partition {}", out.len() + 1);
        }
        out.push(region_at(dev, first * ss, (last - first + 1) * ss, &name, false));
    }
    Some(out)
}

/// Resolve a boot sector found at `at` to its volume start: if it is a backup copy
/// (NTFS last sector, FAT32 sector 6, exFAT sector 12) and the primary exists, return the
/// primary's offset; otherwise `at` itself.
pub fn resolve_boot(dev: &Dev, at: u64) -> Option<(u64, FsKind)> {
    let b = sector(dev, at, 512);
    let kind = fs::detect(&b)?;
    for start in backup_candidates(&b, at) {
        if start != at && fs::detect(&sector(dev, start, 512)) == Some(kind) {
            return Some((start, kind));
        }
    }
    Some((at, kind))
}

/// Alternative starts for backup boot sectors: NTFS keeps one in the volume's last sector,
/// FAT32 in sector 6, exFAT at sector 12.
pub fn backup_candidates(b: &[u8], at: u64) -> Vec<u64> {
    let mut v = Vec::new();
    match fs::detect(b) {
        Some(FsKind::Ntfs) => {
            let bps = le16(b, 0x0B) as u64;
            let total = le64(b, 0x28);
            if let Some(start) = at.checked_sub(total * bps) {
                v.push(start);
            }
        }
        Some(FsKind::Fat32) => {
            if le16(b, 0x32) == 6 {
                if let Some(s) = at.checked_sub(6 * le16(b, 0x0B) as u64) {
                    v.push(s);
                }
            }
        }
        Some(FsKind::ExFat) => {
            let bps = 1u64 << b[0x6C].min(12);
            if let Some(s) = at.checked_sub(12 * bps) {
                v.push(s);
            }
        }
        _ => {}
    }
    v
}
