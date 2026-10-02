//! File format knowledge: signatures, structure walkers that determine exact file length,
//! validation (is this really an intact file?) and media metadata extraction.

pub mod asf;
pub mod bmp;
pub mod ebml;
pub mod flv;
pub mod gif;
pub mod isobmff;
pub mod jpeg;
pub mod mpeg;
pub mod png;
pub mod psd;
pub mod riff;
pub mod tiff;

use serde::{Deserialize, Serialize};

use crate::device::Reader;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Image,
    Video,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Image => "Images",
            Category::Video => "Videos",
        }
    }
}

macro_rules! formats {
    ($( $v:ident : $cat:ident, $name:literal, $ext:literal, [$($alias:literal),*] ;)*) => {
        #[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Format { $($v),* }
        impl Format {
            pub const ALL: &'static [Format] = &[$(Format::$v),*];
            pub fn category(self) -> Category { match self { $(Format::$v => Category::$cat),* } }
            pub fn name(self) -> &'static str { match self { $(Format::$v => $name),* } }
            pub fn ext(self) -> &'static str { match self { $(Format::$v => $ext),* } }
            pub fn from_ext(ext: &str) -> Option<Format> {
                let e = ext.to_ascii_lowercase();
                $( if e == $ext $(|| e == $alias)* { return Some(Format::$v); } )*
                None
            }
        }
    };
}

formats! {
    Jpeg: Image, "JPEG", "jpg", ["jpeg", "jpe", "jfif"];
    Png: Image, "PNG", "png", ["apng"];
    Gif: Image, "GIF", "gif", [];
    Bmp: Image, "BMP", "bmp", ["dib"];
    Tiff: Image, "TIFF", "tif", ["tiff"];
    WebP: Image, "WebP", "webp", [];
    Heic: Image, "HEIC", "heic", ["heif", "hif"];
    Avif: Image, "AVIF", "avif", [];
    Psd: Image, "Photoshop", "psd", ["psb"];
    Cr2: Image, "Canon CR2", "cr2", [];
    Cr3: Image, "Canon CR3", "cr3", [];
    Nef: Image, "Nikon NEF", "nef", ["nrw"];
    Arw: Image, "Sony ARW", "arw", ["srf", "sr2"];
    Dng: Image, "DNG", "dng", [];
    Orf: Image, "Olympus ORF", "orf", [];
    Rw2: Image, "Panasonic RW2", "rw2", [];
    Raf: Image, "Fujifilm RAF", "raf", [];
    Pef: Image, "Pentax PEF", "pef", [];
    Srw: Image, "Samsung SRW", "srw", [];
    Mp4: Video, "MP4", "mp4", ["f4v"];
    Mov: Video, "QuickTime MOV", "mov", ["qt"];
    M4v: Video, "M4V", "m4v", [];
    ThreeGp: Video, "3GP", "3gp", ["3g2"];
    Avi: Video, "AVI", "avi", ["divx"];
    Mkv: Video, "Matroska MKV", "mkv", [];
    WebM: Video, "WebM", "webm", [];
    Wmv: Video, "Windows Media", "wmv", ["asf"];
    Flv: Video, "Flash Video", "flv", [];
    Mpg: Video, "MPEG", "mpg", ["mpeg", "vob", "m2v", "mpe", "mod", "tod"];
    M2ts: Video, "AVCHD M2TS", "mts", ["m2ts", "m2t"];
    Ts: Video, "MPEG-TS", "ts", [];
}

/// Structural family: formats sharing a container/header, used to verify file-system hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Jpeg,
    Png,
    Gif,
    Bmp,
    Tiff,
    Riff,
    Iso,
    Ebml,
    Asf,
    Flv,
    Mpeg,
    Psd,
    Raf,
}

impl Format {
    pub fn family(self) -> Family {
        use Format::*;
        match self {
            Jpeg => Family::Jpeg,
            Png => Family::Png,
            Gif => Family::Gif,
            Bmp => Family::Bmp,
            Tiff | Cr2 | Nef | Arw | Dng | Orf | Rw2 | Pef | Srw => Family::Tiff,
            WebP | Avi => Family::Riff,
            Heic | Avif | Cr3 | Mp4 | Mov | M4v | ThreeGp => Family::Iso,
            Mkv | WebM => Family::Ebml,
            Wmv => Family::Asf,
            Flv => Family::Flv,
            Mpg | M2ts | Ts => Family::Mpeg,
            Psd => Family::Psd,
            Raf => Family::Raf,
        }
    }
    pub fn is_raw(self) -> bool {
        use Format::*;
        matches!(self, Cr2 | Cr3 | Nef | Arw | Dng | Orf | Rw2 | Raf | Pef | Srw)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MediaInfo {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<f64>,
    pub codec: Option<String>,
    /// Capture time (EXIF DateTimeOriginal, container creation time), unix seconds.
    pub taken: Option<i64>,
    pub camera: Option<String>,
}

impl MediaInfo {
    pub fn is_empty(&self) -> bool {
        self.width.is_none() && self.duration.is_none() && self.taken.is_none() && self.camera.is_none()
    }
    pub fn dims(&self) -> Option<String> {
        Some(format!("{}×{}", self.width?, self.height?))
    }
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(d) = self.dims() {
            parts.push(d);
        }
        if let Some(d) = self.duration {
            parts.push(crate::util::format_duration(d));
        }
        if let Some(c) = &self.codec {
            parts.push(c.clone());
        }
        if let Some(c) = &self.camera {
            parts.push(c.clone());
        }
        parts.join(" · ")
    }
}

#[derive(Clone, Debug)]
pub struct Probe {
    pub format: Format,
    pub len: u64,
    /// Structure validated through to its natural end.
    pub complete: bool,
    pub info: MediaInfo,
    /// Extra detail about damage, if any.
    pub note: Option<String>,
}

impl Probe {
    pub fn new(format: Format, len: u64, complete: bool, info: MediaInfo) -> Probe {
        Probe { format, len, complete, info, note: None }
    }
}

const ASF_HEADER: [u8; 16] =
    [0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C];

/// Identify the format from the first bytes of a file (needs >= 400 bytes for MPEG-TS).
/// Returns the most likely format; `probe` refines it (e.g. TIFF -> NEF).
pub fn identify(h: &[u8]) -> Option<Format> {
    if h.len() < 16 {
        return None;
    }
    match h[0] {
        0xFF if h[1] == 0xD8 && h[2] == 0xFF && h[3] >= 0xC0 && h[3] != 0xFF => return Some(Format::Jpeg),
        0x89 if &h[..8] == b"\x89PNG\r\n\x1a\n" => return Some(Format::Png),
        b'G' if &h[..6] == b"GIF87a" || &h[..6] == b"GIF89a" => return Some(Format::Gif),
        b'B' if h[1] == b'M' && bmp::quick_check(h) => return Some(Format::Bmp),
        b'I' | b'M' => {
            if &h[..4] == b"II*\0" || &h[..4] == b"MM\0*" {
                return Some(if &h[8..10] == b"CR" { Format::Cr2 } else { Format::Tiff });
            }
            if &h[..4] == b"IIRO" || &h[..4] == b"IIRS" || &h[..4] == b"MMOR" {
                return Some(Format::Orf);
            }
            if &h[..4] == b"IIU\0" {
                return Some(Format::Rw2);
            }
        }
        b'R' if &h[..4] == b"RIFF" => {
            return match &h[8..12] {
                b"WEBP" => Some(Format::WebP),
                b"AVI " => Some(Format::Avi),
                _ => None,
            }
        }
        0x1A if h[..4] == [0x1A, 0x45, 0xDF, 0xA3] => return Some(Format::Mkv),
        0x30 if h[..16] == ASF_HEADER => return Some(Format::Wmv),
        b'F' if &h[..4] == b"FLV\x01" => return Some(Format::Flv),
        b'F' if h.starts_with(b"FUJIFILMCCD-RAW") => return Some(Format::Raf),
        b'8' if &h[..6] == b"8BPS\0\x01" || &h[..6] == b"8BPS\0\x02" => return Some(Format::Psd),
        0x00 if h[..4] == [0, 0, 1, 0xBA] => return Some(Format::Mpg),
        _ => {}
    }
    match &h[4..8] {
        b"ftyp" => return isobmff::brand_format(h),
        b"moov" => return Some(Format::Mov),
        b"mdat" | b"wide" | b"free" | b"skip" if isobmff::plausible_box(h) => return Some(Format::Mov),
        _ => {}
    }
    if h.len() >= 400 {
        if h[0] == 0x47 && h[188] == 0x47 && h[376] == 0x47 {
            return Some(Format::Ts);
        }
        if h[4] == 0x47 && h[196] == 0x47 && h[388] == 0x47 {
            return Some(Format::M2ts);
        }
    }
    None
}

/// Strong, unambiguous file headers. Seeing one of these at a sector boundary inside another
/// file's data means that file ended (was truncated / fragmented) before this point.
pub fn is_strong_header(h: &[u8]) -> bool {
    if h.len() < 16 {
        return false;
    }
    (h[0] == 0xFF && h[1] == 0xD8 && h[2] == 0xFF && (h[3] == 0xE0 || h[3] == 0xE1 || h[3] == 0xDB))
        || &h[..8] == b"\x89PNG\r\n\x1a\n"
        || &h[..6] == b"GIF89a"
        || &h[..6] == b"GIF87a"
        || (&h[4..8] == b"ftyp" && u32::from_be_bytes([h[0], h[1], h[2], h[3]]) <= 256 && h[8].is_ascii_alphanumeric())
        || (&h[..4] == b"RIFF" && (&h[8..12] == b"AVI " || &h[8..12] == b"WEBP"))
        || h[..4] == [0x1A, 0x45, 0xDF, 0xA3]
        || h[..16] == ASF_HEADER
        || &h[..8] == b"II*\0\x08\0\0\0"
        || &h[..8] == b"MM\0*\0\0\0\x08"
        || &h[..6] == b"8BPS\0\x01"
}

/// Validate the structure at `off` and determine exact length. `limit` caps the length
/// (bytes available until end of source or end of the known file).
pub fn probe(r: &mut Reader, off: u64, limit: u64) -> Option<Probe> {
    let head = r.bytes(off, 512);
    let fmt = identify(&head)?;
    probe_as(r, off, limit, fmt)
}

pub fn probe_as(r: &mut Reader, off: u64, limit: u64, fmt: Format) -> Option<Probe> {
    let p = match fmt.family() {
        Family::Jpeg => jpeg::probe(r, off, limit),
        Family::Png => png::probe(r, off, limit),
        Family::Gif => gif::probe(r, off, limit),
        Family::Bmp => bmp::probe(r, off, limit),
        Family::Tiff => tiff::probe(r, off, limit),
        Family::Riff => riff::probe(r, off, limit),
        Family::Iso => isobmff::probe(r, off, limit),
        Family::Ebml => ebml::probe(r, off, limit),
        Family::Asf => asf::probe(r, off, limit),
        Family::Flv => flv::probe(r, off, limit),
        Family::Mpeg => mpeg::probe(r, off, limit),
        Family::Psd => psd::probe(r, off, limit),
        Family::Raf => tiff::probe_raf(r, off, limit),
    }?;
    (p.len > 0 && p.len <= limit).then_some(p)
}

/// Scan `[pos, end)` for the first zero-filled or foreign-header sector boundary.
/// Used by stream formats (JPEG entropy data) to detect where data stops belonging to the file.
pub fn sector_break(sector: &[u8]) -> bool {
    sector.len() >= 512 && (sector.iter().all(|&b| b == 0) || is_strong_header(sector))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ext_mapping() {
        assert_eq!(Format::from_ext("JPEG"), Some(Format::Jpeg));
        assert_eq!(Format::from_ext("m2ts"), Some(Format::M2ts));
        assert_eq!(Format::from_ext("txt"), None);
        assert_eq!(Format::Mov.category(), Category::Video);
    }
}
