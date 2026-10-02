//! PNG: chunk walk with CRC verification of every chunk up to IEND.

use super::{Format, MediaInfo, Probe};
use crate::device::Reader;

const MAX_PNG: u64 = 1 << 30;

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    if r.bytes(off, 8) != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let end_limit = off + limit.min(MAX_PNG);
    let mut pos = off + 8;
    let mut info = MediaInfo::default();
    let mut first = true;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if pos + 12 > end_limit {
            return truncated(off, pos, info, first);
        }
        let len = r.u32be(pos)? as u64;
        let typ = r.array::<4>(pos + 4)?;
        if len > 0x7FFF_FFFF || !typ.iter().all(|c| c.is_ascii_alphabetic()) {
            return truncated(off, pos, info, first);
        }
        if first {
            if &typ != b"IHDR" || len != 13 {
                return None;
            }
            info.width = r.u32be(pos + 8);
            info.height = r.u32be(pos + 12);
        }
        if pos + 12 + len > end_limit {
            return truncated(off, pos, info, first);
        }
        // CRC over type + data.
        let mut h = crc32fast::Hasher::new();
        h.update(&typ);
        let mut done = 0u64;
        while done < len {
            let n = ((len - done) as usize).min(buf.len());
            let got = r.read(pos + 8 + done, &mut buf[..n]);
            if got < n {
                return truncated(off, pos, info, first);
            }
            h.update(&buf[..n]);
            done += n as u64;
        }
        let crc = r.u32be(pos + 8 + len)?;
        if crc != h.finalize() {
            if first {
                return None;
            }
            let mut p = Probe::new(Format::Png, pos - off, false, info);
            p.note = Some(format!("Corrupted data in {} chunk", String::from_utf8_lossy(&typ)));
            return Some(p);
        }
        first = false;
        pos += 12 + len;
        if &typ == b"IEND" {
            return Some(Probe::new(Format::Png, pos - off, true, info));
        }
    }
}

fn truncated(off: u64, pos: u64, info: MediaInfo, first: bool) -> Option<Probe> {
    if first {
        return None;
    }
    let mut p = Probe::new(Format::Png, pos - off, false, info);
    p.note = Some("Image data incomplete".into());
    Some(p)
}
