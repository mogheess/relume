//! Writing recovered files out, and reading file content for previews.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use crate::device::{read_tolerant, Dev, ExtentDevice, Reader};
use crate::formats::{self, tiff, Format};
use crate::model::{FoundFile, Origin};
use crate::util::sanitize_name;

#[derive(Clone, Debug)]
pub struct RecoverOptions {
    pub dest: PathBuf,
    /// Recreate original folders (for files with names); otherwise flat per type.
    pub keep_folders: bool,
}

#[derive(Clone, Debug, Default)]
pub struct RecoverProgress {
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current: String,
}

#[derive(Clone, Debug, Default)]
pub struct RecoverReport {
    pub recovered: usize,
    pub failed: Vec<(String, String)>,
    pub bytes: u64,
    pub bad_sectors: u32,
    pub cancelled: bool,
    pub dest: PathBuf,
}

pub fn file_device(dev: &Dev, f: &FoundFile) -> Dev {
    Arc::new(ExtentDevice::new(dev.clone(), f.extents.clone(), f.size))
}

fn target_dir(f: &FoundFile, opts: &RecoverOptions) -> PathBuf {
    let mut d = opts.dest.clone();
    if f.origin == Origin::Carved {
        d.push("Lost Files");
        d.push(sanitize_name(f.format.name()));
        return d;
    }
    if opts.keep_folders {
        let vol = if f.volume.trim().is_empty() { "Volume".to_string() } else { f.volume.trim().to_string() };
        d.push(sanitize_name(&vol));
        for part in f.path.split('\\').filter(|p| !p.is_empty()) {
            d.push(sanitize_name(part));
        }
    } else {
        d.push(f.category().label());
    }
    d
}

fn unique(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{}", e)),
        None => (name.to_string(), String::new()),
    };
    for i in 1.. {
        let p = dir.join(format!("{} ({}){}", stem, i, ext));
        if !p.exists() {
            return p;
        }
    }
    unreachable!()
}

fn write_one(dev: &Dev, f: &FoundFile, opts: &RecoverOptions, cancel: &AtomicBool, on_bytes: &mut dyn FnMut(u64)) -> io::Result<(PathBuf, u32)> {
    write_into(dev, f, &target_dir(f, opts), cancel, on_bytes)
}

/// Write one file into `dir` (created if needed). Returns (path written, unreadable sectors).
pub fn write_into(dev: &Dev, f: &FoundFile, dir: &Path, cancel: &AtomicBool, on_bytes: &mut dyn FnMut(u64)) -> io::Result<(PathBuf, u32)> {
    fs::create_dir_all(dir)?;
    let path = unique(dir, &sanitize_name(&f.name));
    let fd = file_device(dev, f);
    let mut out = io::BufWriter::with_capacity(1 << 20, File::create(&path)?);
    let mut buf = vec![0u8; 1 << 20];
    let mut pos = 0u64;
    let mut bad = 0;
    let mut last2 = [0u8; 2];
    while pos < f.size {
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = fs::remove_file(&path);
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = ((f.size - pos) as usize).min(buf.len());
        let (_, b) = read_tolerant(&*fd, pos, &mut buf[..n]);
        bad += b;
        out.write_all(&buf[..n])?;
        if n >= 2 {
            last2 = [buf[n - 2], buf[n - 1]];
        }
        pos += n as u64;
        on_bytes(n as u64);
    }
    if f.fix_jpeg_eoi && f.format == Format::Jpeg && last2 != [0xFF, 0xD9] {
        out.write_all(&[0xFF, 0xD9])?;
    }
    let file = out.into_inner().map_err(|e| e.into_error())?;
    if let Some(t) = f.modified.or(f.info.taken) {
        if t > 0 {
            let _ = file.set_modified(UNIX_EPOCH + Duration::from_secs(t as u64));
        }
    }
    Ok((path, bad))
}

pub fn recover(dev: &Dev, files: &[FoundFile], opts: &RecoverOptions, cancel: &AtomicBool, progress: &mut dyn FnMut(&RecoverProgress)) -> RecoverReport {
    let mut rep = RecoverReport { dest: opts.dest.clone(), ..Default::default() };
    let mut pr = RecoverProgress { files_total: files.len(), bytes_total: files.iter().map(|f| f.size).sum(), ..Default::default() };
    for f in files {
        if cancel.load(Ordering::Relaxed) {
            rep.cancelled = true;
            break;
        }
        pr.current = f.name.clone();
        progress(&pr);
        let mut bytes_cb = |n: u64| {
            pr.bytes_done += n;
        };
        match write_one(dev, f, opts, cancel, &mut bytes_cb) {
            Ok((_, bad)) => {
                rep.recovered += 1;
                rep.bytes += f.size;
                rep.bad_sectors += bad;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                rep.cancelled = true;
                break;
            }
            Err(e) => rep.failed.push((f.full_path(), e.to_string())),
        }
        pr.files_done += 1;
        progress(&pr);
    }
    rep
}

/// Read up to `max` bytes of a found file.
pub fn read_file(dev: &Dev, f: &FoundFile, max: u64) -> Vec<u8> {
    let fd = file_device(dev, f);
    let n = f.size.min(max) as usize;
    let mut buf = vec![0u8; n];
    let (got, _) = read_tolerant(&*fd, 0, &mut buf);
    buf.truncate(got);
    buf
}

/// Bytes the `image` crate can decode for previewing this file. Camera RAW and container
/// formats return their largest embedded JPEG preview.
pub fn preview_bytes(dev: &Dev, f: &FoundFile, max: u64) -> Result<Vec<u8>, String> {
    use Format::*;
    match f.format {
        Jpeg => {
            let mut b = read_file(dev, f, max);
            if b.len() < 4 || b[..2] != [0xFF, 0xD8] {
                return Err("No image data at this location".into());
            }
            if !b.ends_with(&[0xFF, 0xD9]) {
                b.extend_from_slice(&[0xFF, 0xD9]);
            }
            Ok(b)
        }
        Png | Gif | Bmp | WebP | Tiff => {
            if f.size > max {
                return Err(format!("Too large to preview ({})", crate::util::format_size(f.size)));
            }
            Ok(read_file(dev, f, max))
        }
        Cr2 | Nef | Arw | Dng | Orf | Rw2 | Pef | Srw => {
            let mut r = Reader::with_capacity(file_device(dev, f), 16);
            let t = tiff::walk(&mut r, 0, f.size);
            let best = t.previews.iter().filter(|(o, l)| o + l <= f.size).max_by_key(|(_, l)| *l).copied();
            match best {
                Some((o, l)) if l <= max => {
                    let b = r.bytes(o, l as usize);
                    if b.starts_with(&[0xFF, 0xD8]) { Ok(b) } else { embedded_jpeg(dev, f, max) }
                }
                _ => embedded_jpeg(dev, f, max),
            }
        }
        Raf => {
            let mut r = Reader::with_capacity(file_device(dev, f), 16);
            let o = r.u32be(84).ok_or("Unreadable")? as u64;
            let l = r.u32be(88).ok_or("Unreadable")? as u64;
            if l == 0 || l > max {
                return Err("No embedded preview".into());
            }
            Ok(r.bytes(o, l as usize))
        }
        Heic | Avif => Err(format!("{} preview needs the HEIF Image Extension. Use Open to view it.", f.format.name())),
        _ if f.category() == formats::Category::Video => Err("Video".into()),
        _ => embedded_jpeg(dev, f, max),
    }
}

/// Find the largest complete JPEG stream in the first part of a file (CR3, PSD thumbnails, MJPEG).
fn embedded_jpeg(dev: &Dev, f: &FoundFile, max: u64) -> Result<Vec<u8>, String> {
    let fd = file_device(dev, f);
    let mut r = Reader::with_capacity(fd, 32);
    let window = f.size.min(16 << 20);
    let data = r.bytes(0, window as usize);
    let mut best: Option<(u64, u64)> = None;
    let mut i = 0;
    while let Some(k) = memchr::memmem::find(&data[i..], &[0xFF, 0xD8, 0xFF]) {
        let at = (i + k) as u64;
        if let Some(p) = formats::jpeg::probe(&mut r, at, f.size - at) {
            if p.complete && p.len <= max && best.is_none_or(|b| p.len > b.1) {
                best = Some((at, p.len));
            }
        }
        i += k + 3;
        if i >= data.len() {
            break;
        }
    }
    let (o, l) = best.ok_or("No preview available for this file type")?;
    Ok(r.bytes(o, l as usize))
}

/// Small preview for thumbnail grids: EXIF thumbnail when present (very fast).
pub fn thumbnail_bytes(dev: &Dev, f: &FoundFile, max: u64) -> Result<Vec<u8>, String> {
    if f.format == Format::Jpeg {
        let mut r = Reader::with_capacity(file_device(dev, f), 4);
        if let Some((o, l)) = formats::jpeg::exif_thumbnail(&mut r, 0) {
            if l > 1000 && o + l <= f.size {
                let b = r.bytes(o, l as usize);
                if b.starts_with(&[0xFF, 0xD8]) {
                    return Ok(b);
                }
            }
        }
    }
    preview_bytes(dev, f, max)
}

/// Fully validate a file's structure (reads the whole file).
pub fn deep_verify(dev: &Dev, f: &FoundFile) -> crate::fs::Verification {
    crate::fs::verify(dev, &f.extents, f.size, f.format, true)
}
