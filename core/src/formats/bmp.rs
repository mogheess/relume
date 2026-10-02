//! BMP: header-described size validated against the pixel array geometry.

use super::{Format, MediaInfo, Probe};
use crate::device::Reader;
use crate::util::{le16, le32};

struct Hdr {
    size: u64,
    expected: u64,
    width: u32,
    height: u32,
}

fn parse(h: &[u8]) -> Option<Hdr> {
    if h.len() < 54 || &h[..2] != b"BM" {
        return None;
    }
    let size = le32(h, 2) as u64;
    let data_off = le32(h, 10) as u64;
    let dib = le32(h, 14);
    if !matches!(dib, 12 | 40 | 52 | 56 | 64 | 108 | 124) || data_off < 14 + dib as u64 || data_off > 1 << 20 {
        return None;
    }
    let (w, ht, planes, bpp, comp) = if dib == 12 {
        (le16(h, 18) as i64, le16(h, 20) as i64, le16(h, 22), le16(h, 24), 0)
    } else {
        (le32(h, 18) as i32 as i64, le32(h, 22) as i32 as i64, le16(h, 26), le16(h, 28), le32(h, 30))
    };
    if planes != 1 || !matches!(bpp, 1 | 2 | 4 | 8 | 16 | 24 | 32) || comp > 6 {
        return None;
    }
    if w <= 0 || w > 65535 || ht == 0 || ht.abs() > 65535 {
        return None;
    }
    let row = (w as u64 * bpp as u64).div_ceil(32) * 4;
    let pixels = row * ht.unsigned_abs();
    let expected = data_off + pixels;
    let size = if comp == 0 || comp == 3 || comp == 6 {
        // Uncompressed: geometry is authoritative; tolerate sloppy size fields.
        if size >= expected && size <= expected + 4096 { size } else { expected }
    } else {
        if size <= data_off {
            return None;
        }
        size
    };
    Some(Hdr { size, expected, width: w as u32, height: ht.unsigned_abs() as u32 })
}

pub fn quick_check(h: &[u8]) -> bool {
    parse(h).is_some()
}

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let h = r.bytes(off, 64);
    let hdr = parse(&h)?;
    if hdr.size < 64 {
        return None;
    }
    let _ = hdr.expected;
    let info = MediaInfo { width: Some(hdr.width), height: Some(hdr.height), ..Default::default() };
    if hdr.size > limit {
        let mut p = Probe::new(Format::Bmp, limit, false, info);
        p.note = Some("Pixel data incomplete".into());
        return Some(p);
    }
    Some(Probe::new(Format::Bmp, hdr.size, true, info))
}
