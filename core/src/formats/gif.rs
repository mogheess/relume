//! GIF: header, color tables, extension and image blocks up to the trailer.

use super::{Format, MediaInfo, Probe};
use crate::device::Reader;

const MAX_GIF: u64 = 256 << 20;

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let h = r.bytes(off, 13);
    if h.len() < 13 || !(h.starts_with(b"GIF87a") || h.starts_with(b"GIF89a")) {
        return None;
    }
    let end_limit = off + limit.min(MAX_GIF);
    let w = u16::from_le_bytes([h[6], h[7]]) as u32;
    let ht = u16::from_le_bytes([h[8], h[9]]) as u32;
    if w == 0 || ht == 0 {
        return None;
    }
    let info = MediaInfo { width: Some(w), height: Some(ht), ..Default::default() };
    let mut pos = off + 13;
    if h[10] & 0x80 != 0 {
        pos += 3 * (1u64 << ((h[10] & 7) + 1));
    }
    let mut images = 0;
    loop {
        if pos >= end_limit {
            break;
        }
        let Some(b) = r.u8(pos) else { break };
        match b {
            0x3B => return (images > 0).then(|| Probe::new(Format::Gif, pos + 1 - off, true, info)),
            0x21 => {
                let label = r.u8(pos + 1)?;
                if !matches!(label, 0xF9 | 0xFE | 0x01 | 0xFF) {
                    break;
                }
                pos = skip_sub_blocks(r, pos + 2, end_limit)?;
            }
            0x2C => {
                let d = r.bytes(pos, 10);
                if d.len() < 10 {
                    break;
                }
                pos += 10;
                if d[9] & 0x80 != 0 {
                    pos += 3 * (1u64 << ((d[9] & 7) + 1));
                }
                let lzw = r.u8(pos)?;
                if !(1..=12).contains(&lzw) {
                    break;
                }
                pos = skip_sub_blocks(r, pos + 1, end_limit)?;
                images += 1;
            }
            _ => break,
        }
    }
    if images == 0 {
        return None;
    }
    let mut p = Probe::new(Format::Gif, pos.min(end_limit) - off, false, info);
    p.note = Some("Animation/image data incomplete".into());
    Some(p)
}

fn skip_sub_blocks(r: &mut Reader, mut pos: u64, end_limit: u64) -> Option<u64> {
    loop {
        if pos >= end_limit {
            return Some(end_limit);
        }
        let n = r.u8(pos)? as u64;
        pos += 1 + n;
        if n == 0 {
            return Some(pos);
        }
    }
}
