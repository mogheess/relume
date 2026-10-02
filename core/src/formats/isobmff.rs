//! ISO base media (MP4, MOV, M4V, 3GP, HEIC/HEIF, AVIF, Canon CR3).
//! Walks top-level boxes for exact length, parses `moov` for metadata, and spot-checks that
//! the video sample table actually points at video frames (detects overwritten/fragmented data).

use super::{tiff, Format, MediaInfo, Probe};
use crate::device::Reader;
use crate::util::{be16, be32, be64, mac_to_unix};

const TOP_LEVEL: [&[u8; 4]; 20] = [
    b"moov", b"mdat", b"free", b"skip", b"wide", b"uuid", b"meta", b"moof", b"mfra", b"sidx", b"ssix", b"styp",
    b"prft", b"emsg", b"pdin", b"udta", b"junk", b"pnot", b"PICT", b"Xtra",
];

fn classify(brand: &[u8]) -> Option<Option<Format>> {
    // Some(None) = recognised but not image/video (audio); None = unknown brand.
    Some(Some(match brand {
        b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"hevm" | b"hevs" => Format::Heic,
        b"avif" | b"avis" => Format::Avif,
        b"crx " => Format::Cr3,
        b"qt  " => Format::Mov,
        b"M4V " | b"M4VH" | b"M4VP" => Format::M4v,
        b"M4A " | b"M4B " | b"M4P " | b"F4A " | b"F4B " | b"mp3 " => return Some(None),
        b if b.starts_with(b"3g") => Format::ThreeGp,
        b"isom" | b"iso2" | b"iso3" | b"iso4" | b"iso5" | b"iso6" | b"iso8" | b"iso9" | b"mp41" | b"mp42" | b"avc1"
        | b"dash" | b"MSNV" | b"XAVC" | b"NDAS" | b"mmp4" | b"f4v " | b"MP4 " | b"kddi" | b"CAEP" | b"nvr1"
        | b"FACE" | b"mp71" | b"isml" | b"piff" | b"cmfc" => Format::Mp4,
        _ => return None,
    }))
}

/// Determine format from an `ftyp` box at the start of `h`.
pub fn brand_format(h: &[u8]) -> Option<Format> {
    let size = be32(h, 0) as usize;
    if !(16..=1024).contains(&size) || h.len() < 16 {
        return None;
    }
    let major = &h[8..12];
    if !major.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
        return None;
    }
    let compat: Vec<&[u8]> = h[16..size.min(h.len())].chunks_exact(4).collect();
    if major == b"mif1" || major == b"msf1" {
        if compat.iter().any(|c| *c == b"avif" || *c == b"avis") {
            return Some(Format::Avif);
        }
        return Some(Format::Heic);
    }
    match classify(major) {
        Some(f) => f,
        None => {
            // Unknown major brand: fall back to compatible brands, else generic MP4.
            for c in &compat {
                if let Some(f) = classify(c) {
                    return f;
                }
            }
            Some(Format::Mp4)
        }
    }
}

pub fn plausible_box(h: &[u8]) -> bool {
    let size = be32(h, 0);
    size >= 8 && h[4..8].iter().all(|c| c.is_ascii_lowercase())
}

struct TopBox {
    typ: [u8; 4],
    pos: u64,
    size: u64,
}

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let head = r.bytes(off, 512);
    if head.len() < 16 {
        return None;
    }
    let has_ftyp = &head[4..8] == b"ftyp";
    let mut fmt = if has_ftyp { brand_format(&head)? } else { Format::Mov };
    let end_limit = off + limit;
    let mut pos = off;
    let mut boxes: Vec<TopBox> = Vec::new();
    let mut truncated = false;
    while pos + 8 <= end_limit {
        let Some(h) = r.array::<16>(pos).or_else(|| r.array::<8>(pos).map(|a| {
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&a);
            b
        })) else {
            break;
        };
        let typ: [u8; 4] = h[4..8].try_into().unwrap();
        let first = boxes.is_empty();
        if first && has_ftyp {
            if &typ != b"ftyp" {
                return None;
            }
        } else if !TOP_LEVEL.contains(&&typ) {
            break;
        }
        let mut size = be32(&h, 0) as u64;
        if size == 1 {
            size = be64(&h, 8);
            if size < 16 {
                break;
            }
        } else if size == 0 {
            // Box extends to end of file: only legal for the last box (usually mdat).
            size = (end_limit - pos).min(4 << 30);
            truncated = true;
        } else if size < 8 {
            break;
        }
        if pos + size > end_limit {
            boxes.push(TopBox { typ, pos, size: end_limit - pos });
            pos = end_limit;
            truncated = true;
            break;
        }
        boxes.push(TopBox { typ, pos, size });
        pos += size;
        if boxes.len() > 100_000 || truncated {
            break;
        }
    }
    if boxes.is_empty() {
        return None;
    }
    let has = |t: &[u8; 4]| boxes.iter().any(|b| &b.typ == t);
    let has_moov = has(b"moov");
    let has_data = has(b"mdat") || has(b"moof");
    if !has_ftyp && !has_moov {
        return None; // weak signature without an index: not a recognisable file
    }
    if !has_ftyp && boxes.len() < 2 {
        return None;
    }
    let len = pos - off;
    let mut info = MediaInfo::default();
    let mut note = None;
    let mut complete = !truncated;

    if fmt.category() == super::Category::Image && fmt != Format::Cr3 {
        // HEIC / AVIF: metadata in a top-level 'meta' box.
        if let Some(m) = boxes.iter().find(|b| &b.typ == b"meta") {
            if m.size <= 16 << 20 {
                let data = r.bytes(m.pos, m.size as usize);
                if let Some((w, h)) = find_ispe(&data) {
                    info.width = Some(w);
                    info.height = Some(h);
                }
                if let Some(t) = heif_exif(r, off, &data) {
                    info.taken = t.date;
                    info.camera = t.camera();
                }
            }
        } else {
            complete = false;
        }
        if !has_data {
            complete = false;
        }
    } else {
        let moov = boxes.iter().find(|b| &b.typ == b"moov");
        match moov {
            Some(m) if m.size <= 256 << 20 => {
                let data = r.bytes(m.pos, m.size as usize);
                let mut mi = MoovInfo::default();
                parse_moov(&data[8.min(data.len())..], &mut mi);
                info.duration = mi.duration;
                info.taken = mi.created;
                info.width = mi.width;
                info.height = mi.height;
                info.codec = mi.codec.as_ref().map(codec_name);
                if fmt == Format::Cr3 {
                    for tag in [b"CMT1", b"CMT2"] {
                        if let Some(i) = data.windows(4).position(|w| w == tag) {
                            if i >= 4 {
                                let sz = be32(&data, i - 4) as u64;
                                let t = tiff::walk(r, m.pos + i as u64 + 4, sz.saturating_sub(8));
                                if t.date.is_some() {
                                    info.taken = t.date;
                                }
                                if info.camera.is_none() {
                                    info.camera = t.camera();
                                }
                            }
                        }
                    }
                } else {
                    if !mi.has_video && mi.parsed_tracks > 0 {
                        return None; // audio-only container
                    }
                    if mi.has_video && !mi.chunk_offsets.is_empty() {
                        match verify_samples(r, off, len, &mi) {
                            Verify::Ok => {}
                            Verify::Bad(at) => {
                                complete = false;
                                note = Some(if at == 0 {
                                    "Video frames overwritten or relocated".to_string()
                                } else {
                                    format!("Video partially overwritten (~{}% intact)", at)
                                });
                            }
                            Verify::Unknown => {}
                        }
                    }
                }
                if !has_data {
                    complete = false;
                    note.get_or_insert_with(|| "Video data (mdat) missing".into());
                }
            }
            Some(_) => {}
            None => {
                complete = false;
                note = Some("Video index (moov) missing, needs repair".into());
            }
        }
    }
    if fmt == Format::Mp4 && info.codec.is_none() && !has_moov {
        fmt = Format::Mp4;
    }
    if truncated && note.is_none() {
        note = Some("File truncated".into());
    }
    let mut p = Probe::new(fmt, len, complete, info);
    p.note = note;
    Some(p)
}

/// Iterate child boxes of `data` (box payload).
fn children(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p + 8 <= data.len() {
        let mut size = be32(data, p) as usize;
        let typ: [u8; 4] = data[p + 4..p + 8].try_into().unwrap();
        let mut hdr = 8;
        if size == 1 {
            if p + 16 > data.len() {
                break;
            }
            size = be64(data, p + 8) as usize;
            hdr = 16;
        } else if size == 0 {
            size = data.len() - p;
        }
        if size < hdr || p + size > data.len() {
            break;
        }
        out.push((typ, &data[p + hdr..p + size]));
        p += size;
    }
    out
}

#[derive(Default)]
struct MoovInfo {
    duration: Option<f64>,
    created: Option<i64>,
    width: Option<u32>,
    height: Option<u32>,
    codec: Option<[u8; 4]>,
    has_video: bool,
    parsed_tracks: u32,
    nal_len: usize,
    chunk_offsets: Vec<u64>,
}

fn parse_moov(data: &[u8], mi: &mut MoovInfo) {
    for (typ, c) in children(data) {
        match &typ {
            b"mvhd" if c.len() >= 32 => {
                let (created, timescale, duration) = if c[0] == 1 {
                    (be64(c, 4), be32(c, 20) as u64, be64(c, 24))
                } else {
                    (be32(c, 4) as u64, be32(c, 12) as u64, be32(c, 16) as u64)
                };
                mi.created = mac_to_unix(created);
                if timescale > 0 && duration > 0 && duration != u32::MAX as u64 {
                    mi.duration = Some(duration as f64 / timescale as f64);
                }
            }
            b"trak" => {
                mi.parsed_tracks += 1;
                let mut t = TrakInfo::default();
                parse_trak(c, &mut t);
                if t.handler == *b"vide" && !mi.has_video {
                    mi.has_video = true;
                    mi.codec = t.codec;
                    mi.width = t.width.or(t.entry_w);
                    mi.height = t.height.or(t.entry_h);
                    mi.nal_len = t.nal_len;
                    mi.chunk_offsets = t.chunk_offsets;
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct TrakInfo {
    handler: [u8; 4],
    codec: Option<[u8; 4]>,
    width: Option<u32>,
    height: Option<u32>,
    entry_w: Option<u32>,
    entry_h: Option<u32>,
    nal_len: usize,
    chunk_offsets: Vec<u64>,
}

fn parse_trak(data: &[u8], t: &mut TrakInfo) {
    for (typ, c) in children(data) {
        match &typ {
            b"tkhd" => {
                let o = if c.first() == Some(&1) { 88 } else { 76 };
                if c.len() >= o + 8 {
                    let w = be32(c, o) >> 16;
                    let h = be32(c, o + 4) >> 16;
                    if w > 0 && h > 0 {
                        t.width = Some(w);
                        t.height = Some(h);
                    }
                }
            }
            b"mdia" | b"minf" | b"stbl" | b"edts" => parse_trak(c, t),
            // QuickTime has a second (data) handler inside minf: keep the media handler.
            b"hdlr" if c.len() >= 12 && t.handler == [0; 4] => t.handler = c[8..12].try_into().unwrap(),
            b"stsd" if c.len() >= 16 => {
                let entry = &c[8..];
                let fourcc: [u8; 4] = entry[4..8].try_into().unwrap();
                t.codec = Some(fourcc);
                if entry.len() >= 36 {
                    t.entry_w = Some(be16(entry, 32) as u32).filter(|v| *v > 0);
                    t.entry_h = Some(be16(entry, 34) as u32).filter(|v| *v > 0);
                }
                t.nal_len = 4;
                if entry.len() > 86 {
                    let esize = (be32(entry, 0) as usize).min(entry.len());
                    for (bt, bc) in children(&entry[86..esize.max(86)]) {
                        if &bt == b"avcC" && bc.len() > 4 {
                            t.nal_len = (bc[4] & 3) as usize + 1;
                        } else if &bt == b"hvcC" && bc.len() > 21 {
                            t.nal_len = (bc[21] & 3) as usize + 1;
                        }
                    }
                }
            }
            b"stco" if c.len() >= 8 => {
                let n = (be32(c, 4) as usize).min((c.len() - 8) / 4);
                t.chunk_offsets = (0..n).map(|i| be32(c, 8 + i * 4) as u64).collect();
            }
            b"co64" if c.len() >= 8 => {
                let n = (be32(c, 4) as usize).min((c.len() - 8) / 8);
                t.chunk_offsets = (0..n).map(|i| be64(c, 8 + i * 8)).collect();
            }
            _ => {}
        }
    }
}

fn codec_name(c: &[u8; 4]) -> String {
    match c {
        b"avc1" | b"avc3" => "H.264".into(),
        b"hvc1" | b"hev1" => "HEVC".into(),
        b"av01" => "AV1".into(),
        b"vp09" => "VP9".into(),
        b"mp4v" => "MPEG-4".into(),
        b"s263" | b"h263" => "H.263".into(),
        b"jpeg" | b"mjpa" | b"mjpb" => "MJPEG".into(),
        b"apch" | b"apcn" | b"apcs" | b"apco" | b"ap4h" | b"ap4x" => "ProRes".into(),
        b"dvh1" | b"dvhe" => "Dolby Vision".into(),
        _ => String::from_utf8_lossy(c).trim().to_string(),
    }
}

enum Verify {
    Ok,
    /// Percentage position of the first bad sample check.
    Bad(u32),
    Unknown,
}

/// Spot-check that chunk offsets of the video track point to plausible frame data.
fn verify_samples(r: &mut Reader, off: u64, len: u64, mi: &MoovInfo) -> Verify {
    let n = mi.chunk_offsets.len();
    let codec = mi.codec.unwrap_or(*b"    ");
    let mut idx: Vec<usize> = [0, n / 8, n / 4, n / 2, 3 * n / 4, 7 * n / 8, n - 1].to_vec();
    idx.dedup();
    let mut checked = 0;
    for &i in &idx {
        let co = mi.chunk_offsets[i];
        let pct = (i * 100 / n.max(1)) as u32;
        if co + 8 > len {
            return Verify::Bad(pct);
        }
        let Some(b) = r.array::<8>(off + co) else { return Verify::Bad(pct) };
        let ok = match &codec {
            b"avc1" | b"avc3" | b"hvc1" | b"hev1" | b"dvh1" | b"dvhe" => {
                let nl = mi.nal_len.clamp(1, 4);
                let mut l = 0u64;
                for k in 0..nl {
                    l = (l << 8) | b[k] as u64;
                }
                let hdr = b[nl];
                let hevc = codec[0] != b'a';
                l > 0
                    && l < 64 << 20
                    && hdr & 0x80 == 0
                    && if hevc { (hdr >> 1) & 0x3F <= 40 } else { (1..=23).contains(&(hdr & 0x1F)) }
            }
            b"mp4v" => b[..3] == [0, 0, 1],
            b"jpeg" | b"mjpa" | b"mjpb" => b[..2] == [0xFF, 0xD8],
            b"apch" | b"apcn" | b"apcs" | b"apco" | b"ap4h" | b"ap4x" => &b[4..8] == b"icpf",
            _ => return Verify::Unknown,
        };
        if !ok {
            return Verify::Bad(pct);
        }
        checked += 1;
    }
    if checked > 0 { Verify::Ok } else { Verify::Unknown }
}

/// Find the largest `ispe` (image spatial extents) property in a HEIF meta box.
fn find_ispe(data: &[u8]) -> Option<(u32, u32)> {
    let mut best: Option<(u32, u32)> = None;
    let mut i = 0;
    while let Some(k) = data[i..].windows(4).position(|w| w == b"ispe") {
        let p = i + k;
        if p + 16 <= data.len() {
            let w = be32(data, p + 8);
            let h = be32(data, p + 12);
            if w > 0 && h > 0 && w < 100_000 && h < 100_000 && best.is_none_or(|(bw, bh)| (w as u64 * h as u64) > (bw as u64 * bh as u64)) {
                best = Some((w, h));
            }
        }
        i = p + 4;
    }
    best
}

/// HEIF stores EXIF as an item: locate the 'Exif' item via iinf/iloc and parse it.
fn heif_exif(r: &mut Reader, off: u64, meta: &[u8]) -> Option<tiff::TiffInfo> {
    if meta.len() < 12 {
        return None;
    }
    let body = &meta[12..]; // skip box header + FullBox version/flags
    let mut exif_id = None;
    let mut iloc = None;
    for (typ, c) in children(body) {
        match &typ {
            b"iinf" if c.len() > 6 => {
                let start = if c[0] == 0 { 6 } else { 8 };
                for (it, ic) in children(&c[start.min(c.len())..]) {
                    if &it == b"infe" && ic.len() >= 12 && ic[0] >= 2 {
                        let (id, ty) = if ic[0] == 2 {
                            (be16(ic, 4) as u32, &ic[8..12])
                        } else {
                            (be32(ic, 4), &ic[10..14.min(ic.len())])
                        };
                        if ty == b"Exif" {
                            exif_id = Some(id);
                        }
                    }
                }
            }
            b"iloc" => iloc = Some(c),
            _ => {}
        }
    }
    let (id, c) = (exif_id?, iloc?);
    let version = c[0];
    let sizes = c.get(4)?;
    let (off_size, len_size, base_size) = ((sizes >> 4) as usize, (sizes & 15) as usize, (c.get(5)? >> 4) as usize);
    let idx_size = if version == 1 || version == 2 { (c[5] & 15) as usize } else { 0 };
    let mut p = 6;
    let count = if version < 2 {
        let v = be16(c, p) as usize;
        p += 2;
        v
    } else {
        let v = be32(c, p) as usize;
        p += 4;
        v
    };
    let rd = |p: &mut usize, n: usize| -> Option<u64> {
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 8) | *c.get(*p)? as u64;
            *p += 1;
        }
        Some(v)
    };
    for _ in 0..count.min(4096) {
        let item = if version < 2 { rd(&mut p, 2)? } else { rd(&mut p, 4)? } as u32;
        if version == 1 || version == 2 {
            p += 2;
        }
        p += 2; // data reference index
        let base = rd(&mut p, base_size)?;
        let ext_count = rd(&mut p, 2)?;
        let mut first: Option<(u64, u64)> = None;
        for _ in 0..ext_count {
            if idx_size > 0 {
                rd(&mut p, idx_size)?;
            }
            let eo = rd(&mut p, off_size)?;
            let el = rd(&mut p, len_size)?;
            first.get_or_insert((base + eo, el));
        }
        if item == id {
            let (eo, el) = first?;
            // Payload: 4-byte offset to TIFF header, then (usually) "Exif\0\0".
            let skip = r.u32be(off + eo)? as u64;
            let tiff_at = off + eo + 4 + skip;
            return Some(tiff::walk(r, tiff_at, el.saturating_sub(4 + skip)));
        }
    }
    None
}
