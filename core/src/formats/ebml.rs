//! Matroska / WebM (EBML): segment size + level-1 element walk, metadata from Info/Tracks.

use super::{Format, MediaInfo, Probe};
use crate::device::Reader;

const SEGMENT: u32 = 0x1853_8067;
const CLUSTER: u32 = 0x1F43_B675;
const LEVEL1: [u32; 9] = [0x114D_9B74, 0x1549_A966, 0x1654_AE6B, CLUSTER, 0x1C53_BB6B, 0x1043_A770, 0x1254_C367, 0x1941_A469, 0xEC];
const CLUSTER_CHILDREN: [u32; 8] = [0xE7, 0xA7, 0xAB, 0xA3, 0xA0, 0xAF, 0x5854, 0xBF];

/// Read an element ID (with marker bits) -> (id, length).
fn read_id(r: &mut Reader, pos: u64) -> Option<(u32, u64)> {
    let b = r.u8(pos)?;
    let len = b.leading_zeros() as u64 + 1;
    if len > 4 {
        return None;
    }
    let mut v = b as u32;
    for i in 1..len {
        v = (v << 8) | r.u8(pos + i)? as u32;
    }
    Some((v, len))
}

/// Read a data size vint -> (value or None for unknown size, length).
fn read_size(r: &mut Reader, pos: u64) -> Option<(Option<u64>, u64)> {
    let b = r.u8(pos)?;
    if b == 0 {
        return None;
    }
    let len = b.leading_zeros() as u64 + 1;
    let mut v = (b as u64) & ((1u64 << (8 - len)) - 1);
    let mut all_ones = v == (1u64 << (8 - len)) - 1;
    for i in 1..len {
        let x = r.u8(pos + i)?;
        all_ones &= x == 0xFF;
        v = (v << 8) | x as u64;
    }
    Some((if all_ones { None } else { Some(v) }, len))
}

fn elem(r: &mut Reader, pos: u64) -> Option<(u32, Option<u64>, u64)> {
    let (id, il) = read_id(r, pos)?;
    let (sz, sl) = read_size(r, pos + il)?;
    Some((id, sz, il + sl))
}

fn uint(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |a, &x| (a << 8) | x as u64)
}

/// Iterate children inside an in-memory element payload.
fn mem_children(data: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < data.len() {
        let b = data[p];
        let il = b.leading_zeros() as usize + 1;
        if il > 4 || p + il >= data.len() {
            break;
        }
        let id = uint(&data[p..p + il]) as u32;
        let sb = data[p + il];
        if sb == 0 {
            break;
        }
        let sl = sb.leading_zeros() as usize + 1;
        if p + il + sl > data.len() {
            break;
        }
        let mut sz = (sb as u64) & ((1u64 << (8 - sl)) - 1);
        for &x in &data[p + il + 1..p + il + sl] {
            sz = (sz << 8) | x as u64;
        }
        let start = p + il + sl;
        let end = (start as u64 + sz).min(data.len() as u64) as usize;
        out.push((id, &data[start..end]));
        p = end;
    }
    out
}

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let end_limit = off + limit;
    let (id, hsz, hl) = elem(r, off)?;
    if id != 0x1A45_DFA3 {
        return None;
    }
    let hsz = hsz?;
    if hsz > 256 {
        return None;
    }
    let header = r.bytes(off + hl, hsz as usize);
    let mut doctype = String::new();
    for (cid, c) in mem_children(&header) {
        if cid == 0x4282 {
            doctype = String::from_utf8_lossy(c).trim_end_matches('\0').to_string();
        }
    }
    let fmt = match doctype.as_str() {
        "webm" => Format::WebM,
        "matroska" => Format::Mkv,
        _ => return None,
    };
    let seg_pos = off + hl + hsz;
    let (sid, ssz, sl) = elem(r, seg_pos)?;
    if sid != SEGMENT {
        return None;
    }
    let data_start = seg_pos + sl;
    let seg_end = ssz.map(|s| data_start + s);
    let mut info = MediaInfo::default();
    let mut has_video = None;
    let mut timescale = 1_000_000f64;
    let mut duration = None;
    let mut pos = data_start;
    let mut clusters = 0u64;
    let stop = seg_end.unwrap_or(end_limit).min(end_limit);
    let mut broken = false;
    let mut count = 0u64;
    while pos < stop {
        count += 1;
        if count > 2_000_000 {
            break;
        }
        let Some((cid, csz, cl)) = elem(r, pos) else {
            broken = true;
            break;
        };
        if !LEVEL1.contains(&cid) && cid != 0xBF {
            broken = seg_end.is_some();
            break;
        }
        match csz {
            Some(sz) => {
                if (cid == 0x1549_A966 || cid == 0x1654_AE6B) && sz < 4 << 20 {
                    let d = r.bytes(pos + cl, sz as usize);
                    if cid == 0x1549_A966 {
                        for (k, v) in mem_children(&d) {
                            match k {
                                0x2AD7B1 => timescale = uint(v) as f64,
                                0x4489 => {
                                    duration = match v.len() {
                                        4 => Some(f32::from_be_bytes(v.try_into().unwrap()) as f64),
                                        8 => Some(f64::from_be_bytes(v.try_into().unwrap())),
                                        _ => None,
                                    }
                                }
                                0x4461 if v.len() == 8 => {
                                    let ns = i64::from_be_bytes(v.try_into().unwrap());
                                    info.taken = crate::util::plausible(978_307_200 + ns / 1_000_000_000);
                                }
                                _ => {}
                            }
                        }
                    } else {
                        tracks(&d, &mut info, &mut has_video);
                    }
                }
                if cid == CLUSTER {
                    clusters += 1;
                }
                pos += cl + sz;
            }
            None if cid == CLUSTER => {
                clusters += 1;
                pos = skip_unknown_cluster(r, pos + cl, stop);
            }
            None => {
                broken = true;
                break;
            }
        }
    }
    if has_video == Some(false) {
        return None; // audio-only
    }
    if let Some(d) = duration {
        info.duration = Some(d * timescale / 1e9);
    }
    let end = pos.min(stop);
    let complete = !broken && clusters > 0 && seg_end.is_none_or(|e| end >= e && e <= end_limit);
    let mut p = Probe::new(fmt, end - off, complete, info);
    if !complete {
        p.note = Some(if clusters == 0 { "No video data found".into() } else { "Video stream incomplete".into() });
    }
    Some(p)
}

fn skip_unknown_cluster(r: &mut Reader, mut pos: u64, stop: u64) -> u64 {
    while pos < stop {
        let Some((id, sz, l)) = elem(r, pos) else { return pos };
        if LEVEL1.contains(&id) || !CLUSTER_CHILDREN.contains(&id) {
            return pos;
        }
        match sz {
            Some(s) => pos += l + s,
            None => return pos,
        }
    }
    pos
}

fn tracks(d: &[u8], info: &mut MediaInfo, has_video: &mut Option<bool>) {
    for (id, entry) in mem_children(d) {
        if id != 0xAE {
            continue;
        }
        let mut is_video = false;
        let mut codec = None;
        let (mut w, mut h) = (None, None);
        for (k, v) in mem_children(entry) {
            match k {
                0x83 => is_video = uint(v) == 1,
                0x86 => codec = Some(String::from_utf8_lossy(v).trim_end_matches('\0').to_string()),
                0xE0 => {
                    for (vk, vv) in mem_children(v) {
                        match vk {
                            0xB0 => w = Some(uint(vv) as u32),
                            0xBA => h = Some(uint(vv) as u32),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        if is_video && *has_video != Some(true) {
            *has_video = Some(true);
            info.width = w;
            info.height = h;
            info.codec = codec.map(|c| match c.as_str() {
                "V_MPEG4/ISO/AVC" => "H.264".into(),
                "V_MPEGH/ISO/HEVC" => "HEVC".into(),
                "V_VP8" => "VP8".into(),
                "V_VP9" => "VP9".into(),
                "V_AV1" => "AV1".into(),
                _ => c.trim_start_matches("V_").to_string(),
            });
        } else if has_video.is_none() {
            *has_video = Some(false);
        }
    }
}
