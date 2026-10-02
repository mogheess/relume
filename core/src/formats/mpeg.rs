//! MPEG program streams (MPG/VOB) and transport streams (TS, AVCHD MTS/M2TS).

use super::{Format, MediaInfo, Probe};
use crate::device::Reader;

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let h = r.bytes(off, 400);
    if h.len() >= 4 && h[..4] == [0, 0, 1, 0xBA] {
        return program_stream(r, off, limit);
    }
    if h.len() >= 400 {
        if h[0] == 0x47 && h[188] == 0x47 && h[376] == 0x47 {
            return transport_stream(r, off, limit, 188, 0, Format::Ts);
        }
        if h[4] == 0x47 && h[196] == 0x47 && h[388] == 0x47 {
            return transport_stream(r, off, limit, 192, 4, Format::M2ts);
        }
    }
    None
}

fn program_stream(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let end_limit = off + limit;
    let mut pos = off;
    let mut packs = 0u64;
    let mut video = false;
    let mut complete = false;
    while pos + 4 <= end_limit {
        let Some(code) = r.u32be(pos) else { break };
        if code == 0x0000_01BA {
            let b4 = r.u8(pos + 4)?;
            if b4 & 0xC0 == 0x40 {
                let stuff = (r.u8(pos + 13)? & 7) as u64;
                pos += 14 + stuff;
            } else if b4 & 0xF0 == 0x20 {
                pos += 12;
            } else {
                break;
            }
            packs += 1;
        } else if code == 0x0000_01B9 {
            pos += 4;
            complete = true;
            break;
        } else if code >> 8 == 1 && (code & 0xFF) >= 0xBB {
            let len = r.u16be(pos + 4)? as u64;
            if (0xE0..=0xEF).contains(&(code & 0xFF)) {
                video = true;
            }
            pos += 6 + len;
        } else {
            // End of stream without end code (common): the last pack completes the file.
            complete = packs > 0;
            break;
        }
    }
    if packs < 2 || !video {
        return None;
    }
    if pos + 4 > end_limit {
        complete = true; // ran to the end of the available data
    }
    Some(Probe::new(Format::Mpg, pos.min(end_limit) - off, complete, MediaInfo::default()))
}

fn transport_stream(r: &mut Reader, off: u64, limit: u64, stride: u64, sync: u64, fmt: Format) -> Option<Probe> {
    let end_limit = off + limit;
    let mut pos = off;
    let mut packets = 0u64;
    let mut pat = false;
    while pos + stride <= end_limit {
        let (blk, within) = r.view(pos + sync);
        if within >= blk.len() {
            break;
        }
        let mut k = within;
        let mut lost = false;
        while k < blk.len() && pos + stride <= end_limit {
            if blk[k] != 0x47 {
                lost = true;
                break;
            }
            if packets < 256 && k + 2 < blk.len() {
                let pid = (((blk[k + 1] & 0x1F) as u16) << 8) | blk[k + 2] as u16;
                pat |= pid == 0;
            }
            packets += 1;
            pos += stride;
            k += stride as usize;
        }
        if lost {
            break;
        }
    }
    if packets < 64 || !pat {
        return None;
    }
    Some(Probe::new(fmt, pos - off, true, MediaInfo::default()))
}
