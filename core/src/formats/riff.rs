//! RIFF containers: AVI (incl. OpenDML >1 GB AVIX continuation) and WebP.

use super::{Format, MediaInfo, Probe};
use crate::device::Reader;

fn valid_fourcc(b: &[u8]) -> bool {
    b.iter().all(|&c| c.is_ascii_alphanumeric() || c == b' ' || c == b'_')
}

/// Walk the chunks of one RIFF list body. Returns true if chunk headers line up to `end`.
fn chunks_consistent(r: &mut Reader, mut pos: u64, end: u64) -> bool {
    let mut n = 0;
    while pos + 8 <= end {
        let Some(id) = r.array::<4>(pos) else { return false };
        let Some(sz) = r.u32le(pos + 4) else { return false };
        if !valid_fourcc(&id) {
            return false;
        }
        pos += 8 + sz as u64 + (sz as u64 & 1);
        n += 1;
        if n > 100_000 {
            return true;
        }
    }
    pos >= end && pos <= end + 1
}

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let h = r.bytes(off, 256);
    if h.len() < 64 || &h[..4] != b"RIFF" {
        return None;
    }
    let size = u32::from_le_bytes(h[4..8].try_into().ok()?) as u64 + 8;
    match &h[8..12] {
        b"WEBP" => webp(r, off, limit, &h, size),
        b"AVI " => avi(r, off, limit, &h, size),
        _ => None,
    }
}

fn webp(r: &mut Reader, off: u64, limit: u64, h: &[u8], size: u64) -> Option<Probe> {
    let mut info = MediaInfo::default();
    match &h[12..16] {
        b"VP8X" => {
            info.width = Some((u32::from_le_bytes([h[24], h[25], h[26], 0])) + 1);
            info.height = Some((u32::from_le_bytes([h[27], h[28], h[29], 0])) + 1);
        }
        b"VP8 " => {
            if h[23..26] != [0x9d, 0x01, 0x2a] {
                return None;
            }
            info.width = Some(u16::from_le_bytes([h[26], h[27]]) as u32 & 0x3fff);
            info.height = Some(u16::from_le_bytes([h[28], h[29]]) as u32 & 0x3fff);
        }
        b"VP8L" => {
            if h[20] != 0x2f {
                return None;
            }
            let b = u32::from_le_bytes([h[21], h[22], h[23], h[24]]);
            info.width = Some((b & 0x3fff) + 1);
            info.height = Some(((b >> 14) & 0x3fff) + 1);
        }
        _ => return None,
    }
    if size > limit {
        return Some(Probe::new(Format::WebP, limit, false, info));
    }
    let ok = chunks_consistent(r, off + 12, off + size);
    Some(Probe::new(Format::WebP, size, ok, info))
}

fn avi(r: &mut Reader, off: u64, limit: u64, h: &[u8], size: u64) -> Option<Probe> {
    if &h[12..16] != b"LIST" || &h[20..24] != b"hdrl" || &h[24..28] != b"avih" {
        return None;
    }
    let us_per_frame = u32::from_le_bytes(h[32..36].try_into().ok()?) as f64;
    let frames = u32::from_le_bytes(h[48..52].try_into().ok()?) as f64;
    let width = u32::from_le_bytes(h[64..68].try_into().ok()?);
    let height = u32::from_le_bytes(h[68..72].try_into().ok()?);
    let mut info = MediaInfo { width: Some(width).filter(|w| *w > 0), height: Some(height).filter(|h| *h > 0), ..Default::default() };
    // Video codec FourCC from the first 'vids' stream header.
    let hdr = r.bytes(off, 4096);
    if let Some(i) = hdr.windows(8).position(|w| w == b"strhvids") {
        if let Some(cc) = hdr.get(i + 12..i + 16) {
            let s = String::from_utf8_lossy(cc).trim().to_string();
            if !s.is_empty() && cc.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
                info.codec = Some(s);
            }
        }
    }
    if size > limit {
        return Some(Probe::new(Format::Avi, limit, false, info));
    }
    let mut complete = chunks_consistent(r, off + 12, off + size);
    let mut total = size;
    // OpenDML: additional RIFF 'AVIX' chunks follow the first one.
    for _ in 0..4096 {
        let p = off + total;
        let Some(nh) = r.array::<12>(p) else { break };
        if &nh[..4] != b"RIFF" || &nh[8..12] != b"AVIX" {
            break;
        }
        let s = u32::from_le_bytes(nh[4..8].try_into().unwrap()) as u64 + 8;
        if total + s > limit {
            complete = false;
            total = limit;
            break;
        }
        total += s;
    }
    if us_per_frame > 0.0 && frames > 0.0 {
        info.duration = Some(frames * us_per_frame / 1e6);
    }
    let mut p = Probe::new(Format::Avi, total, complete, info);
    if !complete {
        p.note = Some("Video stream incomplete".into());
    }
    Some(p)
}
