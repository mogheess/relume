//! Deep scan: sector-by-sector signature carving. Every hit is validated by the format's
//! structure walker, which also determines the exact file length.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::device::{Dev, Reader};
use crate::formats::{self, Category, Format, Probe};

pub struct CarveOptions<'a> {
    pub categories: &'a [Category],
    /// Jump over files that validated completely (faster; avoids embedded thumbnails).
    pub skip_found: bool,
}

#[derive(Default)]
pub struct CarveStats {
    pub bad_sectors: u32,
    /// Offsets of volume boot sectors seen (primary or backup copies).
    pub boot_sectors: Vec<u64>,
    /// Offsets of FAT directory clusters ("." and ".." entries) — orphaned folders.
    pub fat_dirs: Vec<u64>,
}

fn is_fat_dir(s: &[u8]) -> bool {
    &s[..11] == b".          " && s[11] & 0x10 != 0 && &s[32..43] == b"..         " && s[43] & 0x10 != 0
}

const SECTOR: u64 = 512;

/// Carve `[start, end)` of `dev`.
/// `skip(pos)` returns Some(next) when `pos` lies in space that should not be scanned.
pub fn carve(
    dev: &Dev,
    start: u64,
    end: u64,
    opts: &CarveOptions,
    skip: &dyn Fn(u64) -> Option<u64>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
    found: &mut dyn FnMut(u64, Probe),
) -> CarveStats {
    let mut stats = CarveStats::default();
    let mut r = Reader::with_capacity(dev.clone(), 96);
    let dev_len = dev.len();
    let mut pos = start.div_ceil(SECTOR) * SECTOR;
    let mut last_report = pos;
    while pos + SECTOR <= end {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if let Some(next) = skip(pos) {
            pos = next.max(pos + SECTOR).div_ceil(SECTOR) * SECTOR;
            continue;
        }
        let (blk, within) = r.view(pos);
        if within + SECTOR as usize > blk.len() {
            break;
        }
        let blk_base = pos - within as u64;
        let blk_end = (blk_base + blk.len() as u64).min(end);
        let mut p = pos;
        let mut jumped = None;
        while p + SECTOR <= blk_end {
            if (p % 4096 == 0) && p != pos && skip(p).is_some() {
                break;
            }
            let i = (p - blk_base) as usize;
            let s = &blk[i..i + SECTOR as usize];
            if let Some(fmt) = formats::identify(s) {
                if opts.categories.contains(&fmt.category()) {
                    if let Some(pr) = formats::probe_as(&mut r, p, dev_len - p, fmt) {
                        let len = pr.len;
                        // Stream formats are valid from any packet boundary: always skip past
                        // them, or every 2 KB pack / TS packet run would be reported again.
                        let stream = matches!(pr.format, Format::Mpg | Format::Ts | Format::M2ts);
                        let complete = pr.complete;
                        found(p, pr);
                        if (opts.skip_found && complete || stream) && len > SECTOR {
                            jumped = Some((p + len).div_ceil(SECTOR) * SECTOR);
                            break;
                        }
                    }
                }
            } else if s[510] == 0x55 && s[511] == 0xAA {
                if crate::fs::detect(s).is_some() {
                    stats.boot_sectors.push(p);
                }
            } else if s[0] == b'.' && is_fat_dir(s) && stats.fat_dirs.len() < 1_000_000 {
                stats.fat_dirs.push(p);
            }
            p += SECTOR;
        }
        pos = jumped.unwrap_or(p.max(pos + SECTOR));
        if pos - last_report >= 4 << 20 || pos >= end {
            r.forget_before(pos.saturating_sub(8 << 20));
            progress(pos.min(end));
            last_report = pos;
        }
    }
    progress(pos.min(end));
    stats.bad_sectors = r.bad_sectors;
    stats
}
