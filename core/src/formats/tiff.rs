//! TIFF structure walker: TIFF images, TIFF-based camera RAW (CR2, NEF, ARW, DNG, ORF, RW2,
//! PEF, SRW), EXIF blocks inside JPEG, and Fujifilm RAF.

use std::collections::HashSet;

use super::{Format, MediaInfo, Probe};
use crate::device::Reader;
use crate::util;

#[derive(Default, Debug)]
pub struct TiffInfo {
    /// Furthest byte referenced by the structure, relative to the TIFF header.
    pub max_end: u64,
    pub make: Option<String>,
    pub model: Option<String>,
    pub date: Option<i64>,
    pub width: u32,
    pub height: u32,
    pub dng: bool,
    pub ifds: u32,
    pub has_image_data: bool,
    /// JPEGInterchangeFormat of IFD1 (EXIF thumbnail), relative to TIFF header.
    pub thumbnail: Option<(u64, u64)>,
    /// Embedded JPEG previews (relative offset, length).
    pub previews: Vec<(u64, u64)>,
    pub valid: bool,
}

impl TiffInfo {
    pub fn camera(&self) -> Option<String> {
        let make = self.make.as_deref().unwrap_or("").trim();
        let model = self.model.as_deref().unwrap_or("").trim();
        let first = make.split(' ').next().unwrap_or("").to_lowercase();
        let s = if !first.is_empty() && model.to_lowercase().starts_with(&first) {
            model.to_string()
        } else {
            format!("{} {}", make, model)
        };
        let s = s.trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

struct T<'a> {
    r: &'a mut Reader,
    base: u64,
    limit: u64,
    le: bool,
}

impl T<'_> {
    fn u16(&mut self, o: u64) -> Option<u16> {
        if o + 2 > self.limit {
            return None;
        }
        if self.le { self.r.u16le(self.base + o) } else { self.r.u16be(self.base + o) }
    }
    fn u32(&mut self, o: u64) -> Option<u32> {
        if o + 4 > self.limit {
            return None;
        }
        if self.le { self.r.u32le(self.base + o) } else { self.r.u32be(self.base + o) }
    }
    /// Read array of SHORT/LONG values of a tag.
    fn values(&mut self, typ: u16, count: u32, val_off: u64) -> Vec<u64> {
        let count = count.min(65536) as u64;
        let sz = match typ {
            3 => 2,
            4 | 13 => 4,
            _ => return Vec::new(),
        };
        let data_off = if sz * count <= 4 { val_off } else { self.u32(val_off).unwrap_or(0) as u64 };
        (0..count)
            .filter_map(|i| if sz == 2 { self.u16(data_off + i * 2).map(|v| v as u64) } else { self.u32(data_off + i * 4).map(|v| v as u64) })
            .collect()
    }
    fn ascii(&mut self, count: u32, val_off: u64) -> String {
        let count = count.min(256) as u64;
        let data_off = if count <= 4 { val_off } else { self.u32(val_off).unwrap_or(0) as u64 };
        if data_off + count > self.limit {
            return String::new();
        }
        util::ascii_trim(&self.r.bytes(self.base + data_off, count as usize))
    }
}

fn type_size(t: u16) -> u64 {
    match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

/// Walk a TIFF structure whose header is at `base`. `limit` = bytes available from base.
pub fn walk(r: &mut Reader, base: u64, limit: u64) -> TiffInfo {
    let mut info = TiffInfo::default();
    let Some(h) = r.array::<8>(base) else { return info };
    let le = match &h[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return info,
    };
    let mut t = T { r, base, limit, le };
    let Some(first) = t.u32(4) else { return info };
    let mut queue: Vec<(u64, u8)> = vec![(first as u64, 0)]; // (offset, kind: 0 main chain,1 sub,2 exif)
    let mut seen = HashSet::new();
    let mut chain_index = 0u32;
    info.max_end = 8;
    while let Some((ifd, kind)) = queue.pop() {
        if ifd < 8 || ifd >= limit || !seen.insert(ifd) || seen.len() > 64 {
            continue;
        }
        let Some(n) = t.u16(ifd) else { continue };
        if n == 0 || n > 1000 {
            continue;
        }
        info.ifds += 1;
        let ifd_end = ifd + 2 + n as u64 * 12 + 4;
        if ifd_end > limit {
            continue;
        }
        info.max_end = info.max_end.max(ifd_end);
        let (mut strips, mut strip_counts) = (Vec::new(), Vec::new());
        let (mut jpeg_off, mut jpeg_len) = (None, None);
        let mut w = 0u32;
        let mut hgt = 0u32;
        let mut compression = 0;
        let mut prev_tag = 0u16;
        let mut bad_order = 0;
        for i in 0..n as u64 {
            let e = ifd + 2 + i * 12;
            let (Some(tag), Some(typ), Some(count)) = (t.u16(e), t.u16(e + 2), t.u32(e + 4)) else { break };
            if tag < prev_tag {
                bad_order += 1;
            }
            prev_tag = tag;
            let ts = type_size(typ);
            if ts == 0 {
                bad_order += 1;
                continue;
            }
            let size = ts * count as u64;
            if size > 4 {
                if let Some(o) = t.u32(e + 8) {
                    let end = o as u64 + size;
                    if end <= limit {
                        info.max_end = info.max_end.max(end);
                    }
                }
            }
            let vo = e + 8;
            match tag {
                0x0100 => w = t.values(typ, 1, vo).first().copied().unwrap_or(0) as u32,
                0x0101 => hgt = t.values(typ, 1, vo).first().copied().unwrap_or(0) as u32,
                0x0103 => compression = t.values(typ, 1, vo).first().copied().unwrap_or(0),
                0x010F => info.make = Some(t.ascii(count, vo)).filter(|s| !s.is_empty()),
                0x0110 => info.model = Some(t.ascii(count, vo)).filter(|s| !s.is_empty()),
                0x0132 if info.date.is_none() => {
                    let s = t.ascii(count, vo);
                    info.date = util::exif_date(s.as_bytes());
                }
                0x9003 => {
                    let s = t.ascii(count, vo);
                    if let Some(d) = util::exif_date(s.as_bytes()) {
                        info.date = Some(d);
                    }
                }
                0x0111 => strips = t.values(typ, count, vo),
                0x0117 => strip_counts = t.values(typ, count, vo),
                0x0144 => strips = t.values(typ, count, vo),
                0x0145 => strip_counts = t.values(typ, count, vo),
                0x0201 => jpeg_off = t.values(typ, 1, vo).first().copied(),
                0x0202 => jpeg_len = t.values(typ, 1, vo).first().copied(),
                0x014A | 0x8769 | 0x8825 | 0xA005 => {
                    for v in t.values(if typ == 13 { 4 } else { typ }, count, vo) {
                        queue.push((v, if tag == 0x014A { 1 } else { 2 }));
                    }
                }
                0xC612 => info.dng = true,
                _ => {}
            }
        }
        if bad_order > n as u32 / 2 + 1 {
            continue; // garbage IFD
        }
        info.valid = true;
        if w > info.width {
            info.width = w;
            info.height = hgt;
        }
        for (o, c) in strips.iter().zip(strip_counts.iter()) {
            let end = o + c;
            if end <= limit && *c > 0 {
                info.max_end = info.max_end.max(end);
                info.has_image_data = true;
            }
        }
        if strips.len() == 1 && strip_counts.len() == 1 && (compression == 6 || compression == 7) {
            info.previews.push((strips[0], strip_counts[0]));
        }
        if let (Some(o), Some(l)) = (jpeg_off, jpeg_len) {
            if o + l <= limit && l > 0 {
                info.max_end = info.max_end.max(o + l);
                info.has_image_data = true;
                info.previews.push((o, l));
                if kind == 0 && chain_index == 1 {
                    info.thumbnail = Some((o, l));
                }
            }
        }
        if kind == 0 {
            chain_index += 1;
            if let Some(next) = t.u32(ifd + 2 + n as u64 * 12) {
                if next != 0 {
                    queue.push((next as u64, 0));
                }
            }
        }
    }
    info
}

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let h = r.bytes(off, 16);
    if h.len() < 16 {
        return None;
    }
    let magic_fmt = match &h[..4] {
        b"IIRO" | b"IIRS" | b"MMOR" => Some(Format::Orf),
        b"IIU\0" => Some(Format::Rw2),
        b"II*\0" | b"MM\0*" => None,
        _ => return None,
    };
    let info = walk(r, off, limit);
    if !info.valid || info.ifds == 0 || (!info.has_image_data && info.width == 0) {
        return None;
    }
    let make = info.make.clone().unwrap_or_default().to_ascii_uppercase();
    let fmt = magic_fmt.unwrap_or_else(|| {
        if &h[8..10] == b"CR" {
            Format::Cr2
        } else if info.dng {
            Format::Dng
        } else if make.starts_with("NIKON") {
            Format::Nef
        } else if make.starts_with("SONY") {
            Format::Arw
        } else if make.starts_with("PENTAX") || make.starts_with("RICOH") {
            Format::Pef
        } else if make.starts_with("SAMSUNG") {
            Format::Srw
        } else {
            Format::Tiff
        }
    });
    let mut len = info.max_end;
    // RW2 raw data length is not described by standard tags; it runs to the end of the file.
    let complete = fmt != Format::Rw2;
    if fmt == Format::Rw2 {
        len = len.max(raw_tail(r, off, len, limit));
    }
    let mi = MediaInfo {
        width: (info.width > 0).then_some(info.width),
        height: (info.height > 0).then_some(info.height),
        taken: info.date,
        camera: info.camera(),
        ..Default::default()
    };
    Some(Probe::new(fmt, len.min(limit), complete, mi))
}

/// Estimate where unstructured trailing data ends: first zeroed / foreign sector.
fn raw_tail(r: &mut Reader, off: u64, from: u64, limit: u64) -> u64 {
    let mut pos = (off + from).div_ceil(512) * 512;
    let end = off + limit.min(from + (128 << 20));
    while pos + 512 <= end {
        let s = r.bytes(pos, 512);
        if super::sector_break(&s) {
            break;
        }
        pos += 512;
    }
    pos - off
}

/// Fujifilm RAF: fixed header with offsets to the embedded JPEG and CFA data.
pub fn probe_raf(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    if !r.bytes(off, 16).starts_with(b"FUJIFILMCCD-RAW") {
        return None;
    }
    let mut end = 0u64;
    for o in [84u64, 92, 100] {
        let a = r.u32be(off + o)? as u64;
        let l = r.u32be(off + o + 4)? as u64;
        if a + l <= limit {
            end = end.max(a + l);
        }
    }
    if end < 1024 {
        return None;
    }
    let model = util::ascii_trim(&r.bytes(off + 28, 32));
    let jpeg_off = r.u32be(off + 84)? as u64;
    let mut info = MediaInfo { camera: (!model.is_empty()).then(|| format!("Fujifilm {}", model)), ..Default::default() };
    // Pull EXIF data from the embedded JPEG.
    if let Some(p) = super::jpeg::probe(r, off + jpeg_off, limit.saturating_sub(jpeg_off)) {
        info.taken = p.info.taken;
    }
    Some(Probe::new(Format::Raf, end, true, info))
}
