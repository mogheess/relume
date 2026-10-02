//! Block-level read access to disks, volumes, image files and virtual file streams.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

use crate::model::Extent;

/// Random-access, read-only byte source.
pub trait BlockDevice: Send + Sync {
    /// Read up to `buf.len()` bytes at `offset`. Returns bytes read (short only at end of device).
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
    fn len(&self) -> u64;
    fn sector_size(&self) -> u32 {
        512
    }
}

pub type Dev = Arc<dyn BlockDevice>;

/// A file or raw device opened for reading. Raw Windows devices need sector-aligned I/O,
/// which is handled transparently.
pub struct FileDevice {
    file: File,
    len: u64,
    sector: u32,
    aligned: bool,
}

impl FileDevice {
    pub fn open(path: &Path) -> io::Result<FileDevice> {
        let p = path.to_string_lossy();
        if is_raw_device_path(&p) {
            #[cfg(windows)]
            {
                let (file, len, sector) = crate::platform::open_raw(&p)?;
                return Ok(FileDevice { file, len, sector, aligned: true });
            }
        }
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        #[cfg(unix)]
        let (len, aligned) = if len == 0 { (unix_device_len(&file)?, true) } else { (len, false) };
        #[cfg(not(unix))]
        let aligned = false;
        Ok(FileDevice { file, len, sector: 512, aligned })
    }

    fn raw_read(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let mut done = 0;
        while done < buf.len() {
            #[cfg(unix)]
            let n = std::os::unix::fs::FileExt::read_at(&self.file, &mut buf[done..], offset + done as u64)?;
            #[cfg(windows)]
            let n = std::os::windows::fs::FileExt::seek_read(&self.file, &mut buf[done..], offset + done as u64)?;
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(done)
    }
}

/// `\\.\X:`, `\\.\PhysicalDriveN`, `\\?\X:`, `\\?\Volume{…}` are devices; `\\?\D:\disk.img`
/// (long-path syntax for an ordinary file) is not.
pub fn is_raw_device_path(p: &str) -> bool {
    if let Some(rest) = p.strip_prefix(r"\\.\") {
        return !rest.contains('\\');
    }
    if let Some(rest) = p.strip_prefix(r"\\?\") {
        let rest = rest.trim_end_matches('\\');
        let drive = rest.len() == 2 && rest.as_bytes()[1] == b':';
        return drive || (rest.starts_with("Volume{") && !rest.contains('\\')) || rest.starts_with("GLOBALROOT");
    }
    false
}

#[cfg(unix)]
fn unix_device_len(file: &File) -> io::Result<u64> {
    use std::io::{Seek, SeekFrom};
    let mut f = file.try_clone()?;
    f.seek(SeekFrom::End(0))
}

impl BlockDevice for FileDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.len - offset) as usize;
        if !self.aligned {
            return self.raw_read(offset, &mut buf[..want]);
        }
        let ss = self.sector as u64;
        let start = offset / ss * ss;
        let end = (offset + want as u64).div_ceil(ss) * ss;
        if start == offset && end == offset + want as u64 {
            return self.raw_read(offset, &mut buf[..want]);
        }
        let mut tmp = vec![0u8; (end - start) as usize];
        let n = self.raw_read(start, &mut tmp)?;
        let skip = (offset - start) as usize;
        if n <= skip {
            return Ok(0);
        }
        let got = (n - skip).min(want);
        buf[..got].copy_from_slice(&tmp[skip..skip + got]);
        Ok(got)
    }
    fn len(&self) -> u64 {
        self.len
    }
    fn sector_size(&self) -> u32 {
        self.sector
    }
}

/// A window into another device (a partition / volume inside a disk).
pub struct Slice {
    pub inner: Dev,
    pub base: u64,
    pub len: u64,
}

impl BlockDevice for Slice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.len {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.len - offset) as usize;
        self.inner.read_at(self.base + offset, &mut buf[..want])
    }
    fn len(&self) -> u64 {
        self.len
    }
    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }
}

/// Presents a (possibly fragmented) file as a contiguous stream. Extents with no
/// physical location (sparse) read as zeros.
pub struct ExtentDevice {
    pub inner: Dev,
    pub extents: Vec<Extent>,
    pub len: u64,
}

impl ExtentDevice {
    pub fn new(inner: Dev, extents: Vec<Extent>, len: u64) -> ExtentDevice {
        ExtentDevice { inner, extents, len }
    }
}

impl BlockDevice for ExtentDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.len {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.len - offset) as usize;
        let mut done = 0usize;
        let mut logical = 0u64;
        for e in &self.extents {
            if done >= want {
                break;
            }
            let pos = offset + done as u64;
            let e_end = logical + e.len;
            if pos < e_end && pos >= logical {
                let within = pos - logical;
                let n = ((e.len - within) as usize).min(want - done);
                match e.offset {
                    Some(phys) => {
                        let got = read_full(&*self.inner, phys + within, &mut buf[done..done + n])?;
                        if got < n {
                            buf[done + got..done + n].fill(0);
                        }
                    }
                    None => buf[done..done + n].fill(0),
                }
                done += n;
            }
            logical = e_end;
        }
        if done < want {
            // Extents shorter than declared length: pad with zeros.
            buf[done..want].fill(0);
        }
        Ok(want)
    }
    fn len(&self) -> u64 {
        self.len
    }
}

/// Read as much as possible; loops over short reads.
pub fn read_full(dev: &dyn BlockDevice, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    let mut done = 0;
    while done < buf.len() {
        let n = dev.read_at(offset + done as u64, &mut buf[done..])?;
        if n == 0 {
            break;
        }
        done += n;
    }
    Ok(done)
}

/// Read that survives bad sectors: failing regions are retried in smaller pieces and
/// unreadable sectors are zero-filled. Returns (bytes read, bad sector count).
pub fn read_tolerant(dev: &dyn BlockDevice, offset: u64, buf: &mut [u8]) -> (usize, u32) {
    match read_full(dev, offset, buf) {
        Ok(n) => (n, 0),
        Err(_) => {
            let ss = dev.sector_size().max(512) as usize;
            let piece = if buf.len() > 64 * 1024 { 64 * 1024 } else { ss };
            if buf.len() <= ss {
                buf.fill(0);
                let n = (buf.len() as u64).min(dev.len().saturating_sub(offset)) as usize;
                return (n, 1);
            }
            let mut bad = 0;
            let mut total = 0;
            let mut pos = 0;
            while pos < buf.len() {
                let end = (pos + piece).min(buf.len());
                let (n, b) = read_tolerant(dev, offset + pos as u64, &mut buf[pos..end]);
                bad += b;
                total += n;
                if n < end - pos {
                    break;
                }
                pos = end;
            }
            (total, bad)
        }
    }
}

const BLOCK: usize = 1 << 20;

/// Block-caching reader used by parsers. Cheap repeated small reads, look-ahead friendly.
pub struct Reader {
    dev: Dev,
    blocks: HashMap<u64, Arc<Vec<u8>>>,
    lru: VecDeque<u64>,
    cap: usize,
    pub bad_sectors: u32,
}

impl Reader {
    pub fn new(dev: Dev) -> Reader {
        Reader::with_capacity(dev, 48)
    }
    pub fn with_capacity(dev: Dev, blocks: usize) -> Reader {
        Reader { dev, blocks: HashMap::new(), lru: VecDeque::new(), cap: blocks.max(2), bad_sectors: 0 }
    }
    pub fn len(&self) -> u64 {
        self.dev.len()
    }
    pub fn dev(&self) -> &Dev {
        &self.dev
    }

    fn block(&mut self, idx: u64) -> Arc<Vec<u8>> {
        if let Some(b) = self.blocks.get(&idx) {
            return b.clone();
        }
        let off = idx * BLOCK as u64;
        let len = (BLOCK as u64).min(self.dev.len().saturating_sub(off)) as usize;
        let mut buf = vec![0u8; len];
        let (n, bad) = read_tolerant(&*self.dev, off, &mut buf);
        self.bad_sectors += bad;
        buf.truncate(n);
        let b = Arc::new(buf);
        if self.lru.len() >= self.cap {
            if let Some(old) = self.lru.pop_front() {
                self.blocks.remove(&old);
            }
        }
        self.blocks.insert(idx, b.clone());
        self.lru.push_back(idx);
        b
    }

    /// Drop cached blocks entirely before `offset` (sequential scanning hint).
    pub fn forget_before(&mut self, offset: u64) {
        let limit = offset / BLOCK as u64;
        self.lru.retain(|i| *i >= limit);
        self.blocks.retain(|i, _| *i >= limit);
    }

    /// Read into `buf`; returns number of bytes available (short at end of device).
    pub fn read(&mut self, offset: u64, buf: &mut [u8]) -> usize {
        let mut done = 0;
        while done < buf.len() {
            let pos = offset + done as u64;
            let idx = pos / BLOCK as u64;
            let within = (pos % BLOCK as u64) as usize;
            let b = self.block(idx);
            if within >= b.len() {
                break;
            }
            let n = (b.len() - within).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&b[within..within + n]);
            done += n;
        }
        done
    }

    pub fn bytes(&mut self, offset: u64, len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len];
        let n = self.read(offset, &mut v);
        v.truncate(n);
        v
    }

    /// Exactly `N` bytes or None.
    pub fn array<const N: usize>(&mut self, offset: u64) -> Option<[u8; N]> {
        let mut a = [0u8; N];
        (self.read(offset, &mut a) == N).then_some(a)
    }

    pub fn u8(&mut self, o: u64) -> Option<u8> {
        self.array::<1>(o).map(|a| a[0])
    }
    pub fn u16be(&mut self, o: u64) -> Option<u16> {
        self.array(o).map(u16::from_be_bytes)
    }
    pub fn u32be(&mut self, o: u64) -> Option<u32> {
        self.array(o).map(u32::from_be_bytes)
    }
    pub fn u64be(&mut self, o: u64) -> Option<u64> {
        self.array(o).map(u64::from_be_bytes)
    }
    pub fn u16le(&mut self, o: u64) -> Option<u16> {
        self.array(o).map(u16::from_le_bytes)
    }
    pub fn u32le(&mut self, o: u64) -> Option<u32> {
        self.array(o).map(u32::from_le_bytes)
    }
    pub fn u64le(&mut self, o: u64) -> Option<u64> {
        self.array(o).map(u64::from_le_bytes)
    }

    /// Borrow a cached block slice starting at `offset` (zero-copy view, may be short).
    pub fn view(&mut self, offset: u64) -> (Arc<Vec<u8>>, usize) {
        let idx = offset / BLOCK as u64;
        let within = (offset % BLOCK as u64) as usize;
        (self.block(idx), within)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn raw_paths() {
        use super::is_raw_device_path as r;
        assert!(r(r"\\.\E:"));
        assert!(r(r"\\.\PhysicalDrive2"));
        assert!(r(r"\\?\E:"));
        assert!(r(r"\\?\Volume{0b1c2d3e-0000-0000-0000-100000000000}"));
        assert!(!r(r"\\?\D:\images\card.img"));
        assert!(!r(r"C:\card.img"));
    }
}

pub fn open_path(path: &Path) -> io::Result<Dev> {
    Ok(Arc::new(FileDevice::open(path)?))
}
