//! JPEG: walk markers, skip entropy-coded data, stop at EOI. Detects truncation (foreign
//! headers / zeroed sectors inside scan data) so damaged photos are reported as such.

use super::{sector_break, tiff, Format, MediaInfo, Probe};
use crate::device::Reader;

const MAX_JPEG: u64 = 512 << 20;

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let p = probe_one(r, off, limit)?;
    let mut probe = p;
    // Multi-picture JPEGs (MPF: depth maps, gain maps, Samsung/Apple extras) are appended
    // back-to-back after the primary EOI. Keep them with the file.
    if probe.complete {
        for _ in 0..8 {
            let next = off + probe.len;
            let h = r.bytes(next, 4);
            if h.len() == 4 && h[0] == 0xFF && h[1] == 0xD8 && h[2] == 0xFF && h[3] >= 0xC0 && next % 512 != 0 {
                match probe_one(r, next, limit - probe.len) {
                    Some(sub) if sub.complete => probe.len += sub.len,
                    _ => break,
                }
            } else {
                break;
            }
        }
    }
    Some(probe)
}

fn is_sof(m: u8) -> bool {
    matches!(m, 0xC0..=0xCF) && m != 0xC4 && m != 0xC8 && m != 0xCC
}

fn probe_one(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let end_limit = off + limit.min(MAX_JPEG);
    let h = r.bytes(off, 3);
    if h != [0xFF, 0xD8, 0xFF] {
        return None;
    }
    let mut info = MediaInfo::default();
    let mut pos = off + 2;
    let mut seen_sof = false;
    let mut seen_sos = false;
    let mut segments = 0;
    let (end, complete) = loop {
        if pos + 2 > end_limit {
            break (end_limit.min(pos), false);
        }
        let Some(b) = r.u8(pos) else { break (pos, false) };
        if b != 0xFF {
            break (pos, false);
        }
        let mut m = r.u8(pos + 1)?;
        let mut fills = 0;
        while m == 0xFF && fills < 64 {
            pos += 1;
            fills += 1;
            m = r.u8(pos + 1)?;
        }
        match m {
            0xD9 => break (pos + 2, seen_sof && seen_sos),
            0xD8 | 0x00 => break (pos, false),
            0xD0..=0xD7 | 0x01 => {
                pos += 2;
                continue;
            }
            _ => {}
        }
        // Marker sanity: after the first scan only table/scan/comment markers are expected.
        let allowed = if seen_sos {
            matches!(m, 0xC4 | 0xCC | 0xDA | 0xDB | 0xDC | 0xDD | 0xDE | 0xDF | 0xFE | 0xE0..=0xEF) || is_sof(m)
        } else {
            matches!(m, 0xC0..=0xCF | 0xDA | 0xDB | 0xDC | 0xDD | 0xDE | 0xDF | 0xE0..=0xEF | 0xFE)
        };
        if !allowed {
            break (pos, false);
        }
        let Some(seglen) = r.u16be(pos + 2) else { break (pos, false) };
        if seglen < 2 {
            break (pos, false);
        }
        segments += 1;
        if segments > 4096 {
            break (pos, false);
        }
        let seg_data = pos + 4;
        if is_sof(m) && seglen >= 8 && !seen_sof {
            let height = r.u16be(seg_data + 1)?;
            let width = r.u16be(seg_data + 3)?;
            if width == 0 {
                return None;
            }
            info.width = Some(width as u32);
            info.height = Some(height as u32);
            seen_sof = true;
        } else if m == 0xE1 && seglen > 14 && info.taken.is_none() {
            if r.bytes(seg_data, 6) == b"Exif\0\0" {
                let t = tiff::walk(r, seg_data + 6, seglen as u64 - 8);
                info.taken = t.date;
                info.camera = t.camera();
            }
        }
        pos += 2 + seglen as u64;
        if m == 0xDA {
            if !seen_sof {
                break (pos, false);
            }
            seen_sos = true;
            match scan_entropy(r, pos, end_limit) {
                Some(next) => pos = next,
                None => break (find_data_end(r, pos, end_limit), false),
            }
        }
    };
    if !seen_sof {
        return None;
    }
    if !seen_sos && !complete {
        // Header only, no image data: nothing worth recovering.
        return None;
    }
    let mut p = Probe::new(Format::Jpeg, end - off, complete, info);
    if !complete {
        p.note = Some("Image data incomplete (partially overwritten or fragmented)".into());
    }
    Some(p)
}

/// Skip entropy-coded data starting at `pos`. Returns position of the next real marker,
/// or None when the data stops looking like scan data (zeroed / foreign sector / limit).
fn scan_entropy(r: &mut Reader, mut pos: u64, end_limit: u64) -> Option<u64> {
    loop {
        if pos >= end_limit {
            return None;
        }
        let (blk, within) = r.view(pos);
        if within >= blk.len() {
            return None;
        }
        let data = &blk[within..];
        let mut i = 0usize;
        while i < data.len() {
            let abs = pos + i as u64;
            if abs >= end_limit {
                return None;
            }
            if abs % 512 == 0 && i + 512 <= data.len() && sector_break(&data[i..i + 512]) {
                return None;
            }
            // Find next 0xFF up to the next sector boundary.
            let to_boundary = (512 - (abs % 512)) as usize;
            let stop = (i + to_boundary).min(data.len());
            match memchr::memchr(0xFF, &data[i..stop]) {
                None => {
                    i = stop;
                    continue;
                }
                Some(k) => {
                    let ff = i + k;
                    let next = if ff + 1 < data.len() { data[ff + 1] } else { r.u8(pos + ff as u64 + 1)? };
                    match next {
                        0x00 | 0xD0..=0xD7 | 0xFF => {
                            i = ff + 1;
                        }
                        _ => return Some(pos + ff as u64),
                    }
                }
            }
        }
        pos += data.len() as u64;
    }
}

/// For truncated scans: end at the last sector before the data stops (zero / foreign sector).
fn find_data_end(r: &mut Reader, start: u64, end_limit: u64) -> u64 {
    let mut pos = start.div_ceil(512) * 512;
    while pos + 512 <= end_limit {
        let s = r.bytes(pos, 512);
        if s.len() < 512 || sector_break(&s) {
            return pos.max(start);
        }
        pos += 512;
    }
    end_limit.max(start)
}

/// Extract embedded EXIF thumbnail (offset, len) relative to the JPEG start, if any.
pub fn exif_thumbnail(r: &mut Reader, off: u64) -> Option<(u64, u64)> {
    let mut pos = off + 2;
    for _ in 0..16 {
        if r.u8(pos)? != 0xFF {
            return None;
        }
        let m = r.u8(pos + 1)?;
        let len = r.u16be(pos + 2)? as u64;
        if m == 0xE1 && r.bytes(pos + 4, 6) == b"Exif\0\0" {
            let base = pos + 10;
            let t = tiff::walk(r, base, len.saturating_sub(8));
            let (o, l) = t.thumbnail?;
            return Some((base + o - off, l));
        }
        if m == 0xDA {
            return None;
        }
        pos += 2 + len;
    }
    None
}
