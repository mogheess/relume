//! Photoshop PSD/PSB: section lengths + compressed image data size.

use super::{Format, MediaInfo, Probe};
use crate::device::Reader;

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let h = r.bytes(off, 26);
    if h.len() < 26 || &h[..4] != b"8BPS" {
        return None;
    }
    let ver = u16::from_be_bytes([h[4], h[5]]);
    let psb = ver == 2;
    let channels = u16::from_be_bytes([h[12], h[13]]) as u64;
    let height = u32::from_be_bytes(h[14..18].try_into().ok()?) as u64;
    let width = u32::from_be_bytes(h[18..22].try_into().ok()?) as u64;
    let depth = u16::from_be_bytes([h[22], h[23]]) as u64;
    let mode = u16::from_be_bytes([h[24], h[25]]);
    if !(1..=56).contains(&channels) || height == 0 || width == 0 || !matches!(depth, 1 | 8 | 16 | 32) || mode > 9 {
        return None;
    }
    let mut pos = off + 26;
    let cm = r.u32be(pos)? as u64;
    pos += 4 + cm;
    let res = r.u32be(pos)? as u64;
    pos += 4 + res;
    let lm = if psb { r.u64be(pos)? } else { r.u32be(pos)? as u64 };
    pos += if psb { 8 } else { 4 } + lm;
    if pos - off > limit {
        return None;
    }
    let comp = r.u16be(pos)?;
    pos += 2;
    let info = MediaInfo { width: Some(width as u32), height: Some(height as u32), ..Default::default() };
    let rows = channels * height;
    let data = match comp {
        0 => rows * (width * depth).div_ceil(8),
        1 => {
            let es = if psb { 4 } else { 2 };
            let table = r.bytes(pos, (rows * es) as usize);
            if table.len() as u64 != rows * es {
                return None;
            }
            let sum: u64 = table
                .chunks_exact(es as usize)
                .map(|c| if es == 2 { u16::from_be_bytes([c[0], c[1]]) as u64 } else { u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as u64 })
                .sum();
            rows * es + sum
        }
        _ => {
            let mut p = Probe::new(Format::Psd, (pos - off).min(limit), false, info);
            p.note = Some("ZIP-compressed image data: length estimated".into());
            return Some(p);
        }
    };
    let len = pos + data - off;
    if len > limit {
        return Some(Probe::new(Format::Psd, limit, false, info));
    }
    Some(Probe::new(Format::Psd, len, true, info))
}
