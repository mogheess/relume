//! FLV: tag chain with back-pointers (PreviousTagSize) validated tag by tag.

use super::{Format, MediaInfo, Probe};
use crate::device::Reader;

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let h = r.bytes(off, 13);
    if h.len() < 13 || &h[..4] != b"FLV\x01" {
        return None;
    }
    let flags = h[4];
    if flags & 0x01 == 0 {
        return None; // audio only
    }
    let hdr_len = u32::from_be_bytes(h[5..9].try_into().ok()?) as u64;
    if !(9..=64).contains(&hdr_len) {
        return None;
    }
    let end_limit = off + limit;
    let mut pos = off + hdr_len + 4;
    let mut info = MediaInfo::default();
    let mut tags = 0u64;
    let mut video_tags = 0u64;
    while pos + 15 <= end_limit {
        let Some(t) = r.array::<11>(pos) else { break };
        let typ = t[0] & 0x1F;
        let size = u32::from_be_bytes([0, t[1], t[2], t[3]]) as u64;
        if !matches!(typ, 8 | 9 | 18) || t[8..11] != [0, 0, 0] || pos + 11 + size + 4 > end_limit {
            break;
        }
        let Some(prev) = r.u32be(pos + 11 + size) else { break };
        if prev as u64 != size + 11 {
            break;
        }
        if typ == 18 && tags == 0 && size < 1 << 20 {
            let d = r.bytes(pos + 11, size as usize);
            let num = |key: &[u8]| -> Option<f64> {
                let i = d.windows(key.len()).position(|w| w == key)? + key.len();
                if d.get(i) != Some(&0) {
                    return None;
                }
                Some(f64::from_be_bytes(d.get(i + 1..i + 9)?.try_into().ok()?))
            };
            info.duration = num(b"duration").filter(|v| *v > 0.0);
            info.width = num(b"width").map(|v| v as u32).filter(|v| *v > 0);
            info.height = num(b"height").map(|v| v as u32).filter(|v| *v > 0);
        }
        if typ == 9 {
            video_tags += 1;
        }
        tags += 1;
        pos += 11 + size + 4;
    }
    if video_tags == 0 {
        return None;
    }
    Some(Probe::new(Format::Flv, pos - off, true, info))
}
