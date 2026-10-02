//! ASF / WMV: header objects give exact file size, duration and video stream presence.

use super::{Format, MediaInfo, Probe};
use crate::device::Reader;
use crate::util::{filetime_to_unix, le32, le64};

const FILE_PROPS: [u8; 16] = [0xA1, 0xDC, 0xAB, 0x8C, 0x47, 0xA9, 0xCF, 0x11, 0x8E, 0xE4, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65];
const STREAM_PROPS: [u8; 16] = [0x91, 0x07, 0xDC, 0xB7, 0xB7, 0xA9, 0xCF, 0x11, 0x8E, 0xE6, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65];
const VIDEO_MEDIA: [u8; 16] = [0xC0, 0xEF, 0x19, 0xBC, 0x4D, 0x5B, 0xCF, 0x11, 0xA8, 0xFD, 0x00, 0x80, 0x5F, 0x5C, 0x44, 0x2B];
const DATA_OBJ: [u8; 16] = [0x36, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C];
const INDEXES: [[u8; 16]; 4] = [
    [0x90, 0x08, 0x00, 0x33, 0xB1, 0xE5, 0xCF, 0x11, 0x89, 0xF4, 0x00, 0xA0, 0xC9, 0x03, 0x49, 0xCB],
    [0xD3, 0x29, 0xE2, 0xD6, 0xDA, 0x35, 0xD1, 0x11, 0x90, 0x34, 0x00, 0xA0, 0xC9, 0x03, 0x49, 0xBE],
    [0xF8, 0x03, 0xB1, 0xFE, 0xAD, 0x12, 0x64, 0x4C, 0x84, 0x0F, 0x2A, 0x1D, 0x2F, 0x7A, 0xD4, 0x8C],
    [0xD0, 0x3F, 0xB7, 0x3C, 0x4A, 0x0C, 0x03, 0x48, 0x95, 0x3D, 0xED, 0xF7, 0xB6, 0x22, 0x8F, 0x0C],
];

pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let hsize = r.u64le(off + 16)?;
    if !(30..=16 << 20).contains(&hsize) || hsize > limit {
        return None;
    }
    let hdr = r.bytes(off, hsize as usize);
    let nobj = le32(&hdr, 24);
    let mut p = 30usize;
    let mut info = MediaInfo::default();
    let mut file_size = None;
    let mut has_video = false;
    let mut has_stream = false;
    for _ in 0..nobj.min(1024) {
        if p + 24 > hdr.len() {
            break;
        }
        let guid: [u8; 16] = hdr[p..p + 16].try_into().unwrap();
        let sz = le64(&hdr, p + 16) as usize;
        if sz < 24 || p + sz > hdr.len() {
            break;
        }
        let o = &hdr[p..p + sz];
        if guid == FILE_PROPS && sz >= 104 {
            let fsz = le64(o, 40);
            info.taken = filetime_to_unix(le64(o, 48));
            let play = le64(o, 64) as f64 / 1e7;
            let preroll = le64(o, 80) as f64 / 1e3;
            if play > preroll {
                info.duration = Some(play - preroll);
            }
            let broadcast = le32(o, 88) & 1 != 0;
            if !broadcast && fsz > hsize {
                file_size = Some(fsz);
            }
        } else if guid == STREAM_PROPS && sz >= 78 {
            has_stream = true;
            if o[24..40] == VIDEO_MEDIA {
                has_video = true;
                if sz >= 109 {
                    info.width = Some(le32(o, 78)).filter(|v| *v > 0);
                    info.height = Some(le32(o, 82)).filter(|v| *v > 0);
                    let cc = &o[105..109];
                    if cc.iter().all(|c| c.is_ascii_alphanumeric()) {
                        info.codec = Some(String::from_utf8_lossy(cc).to_string());
                    }
                }
            }
        }
        p += sz;
    }
    if has_stream && !has_video {
        return None; // WMA audio
    }
    // Walk top-level objects after the header.
    let mut pos = off + hsize;
    let mut saw_data = false;
    loop {
        let Some(g) = r.array::<16>(pos) else { break };
        if g != DATA_OBJ && !INDEXES.contains(&g) {
            break;
        }
        let Some(sz) = r.u64le(pos + 16) else { break };
        if sz < 24 || pos + sz > off + limit {
            break;
        }
        saw_data |= g == DATA_OBJ;
        pos += sz;
    }
    let walked = pos - off;
    let (len, complete) = match file_size {
        Some(f) if f <= limit => (f.max(walked), saw_data && walked >= f),
        Some(_) => (limit, false),
        None => (walked, saw_data),
    };
    let mut pr = Probe::new(Format::Wmv, len, complete, info);
    if !complete {
        pr.note = Some("Video stream incomplete".into());
    }
    Some(pr)
}
