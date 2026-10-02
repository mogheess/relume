//! Background decoding of thumbnails and previews straight from the source device (in memory,
//! nothing is written to disk), plus on-demand full integrity verification.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use relume_core::fs::Verification;
use relume_core::model::FoundFile;
use relume_core::recover;
use relume_core::Dev;

const THUMB_PX: u32 = 420;
const LARGE_PX: u32 = 1800;
const MAX_IMAGE_BYTES: u64 = 160 << 20;
const THUMB_CACHE: usize = 600;

enum Job {
    Thumb(u64, Dev, FoundFile),
    Large(u64, Dev, FoundFile),
    Verify(u64, Dev, FoundFile),
}

enum Done {
    Thumb(u64, Result<ColorImage, String>),
    Large(u64, Result<(ColorImage, [u32; 2]), String>),
    Verify(u64, Verification),
}

pub enum Slot {
    Loading,
    Ready(TextureHandle, [u32; 2]),
    Failed(String),
}

struct Queue {
    jobs: Mutex<(VecDeque<Job>, u64)>,
    cv: Condvar,
}

pub struct Previewer {
    ctx: egui::Context,
    q: Arc<Queue>,
    rx: Receiver<Done>,
    thumbs: HashMap<u64, Slot>,
    order: VecDeque<u64>,
    pub large: Option<(u64, Slot)>,
    pub verified: HashMap<u64, Option<Verification>>,
}

fn to_color(img: image::DynamicImage) -> ColorImage {
    let rgba = img.to_rgba8();
    ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
}

fn decode(dev: &Dev, f: &FoundFile, max_px: u32, thumb: bool) -> Result<(ColorImage, [u32; 2]), String> {
    let bytes = if thumb { recover::thumbnail_bytes(dev, f, MAX_IMAGE_BYTES) } else { recover::preview_bytes(dev, f, MAX_IMAGE_BYTES) };
    let bytes = match bytes {
        Ok(b) => b,
        // HEIC/AVIF, or RAW without an embedded preview: let Windows' own codecs try the file.
        Err(e) => return system_decode(&recover::read_file(dev, f, MAX_IMAGE_BYTES), max_px).ok_or(e),
    };
    let img = match image::load_from_memory(&bytes) {
        Ok(i) => i,
        Err(e) => {
            if let Some(r) = system_decode(&bytes, max_px) {
                return Ok(r);
            }
            return Err(if f.health == relume_core::Health::Overwritten {
                "The data at this location was overwritten".to_string()
            } else {
                format!("Can't render a preview ({})", e)
            });
        }
    };
    let dims = [img.width(), img.height()];
    let img = if img.width() > max_px || img.height() > max_px { img.thumbnail(max_px, max_px) } else { img };
    Ok((to_color(img), dims))
}

#[cfg(windows)]
fn system_decode(bytes: &[u8], max_px: u32) -> Option<(ColorImage, [u32; 2])> {
    let (px, w, h, dims) = crate::wic::decode(bytes, max_px).ok()?;
    Some((ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px), dims))
}

#[cfg(not(windows))]
fn system_decode(_bytes: &[u8], _max_px: u32) -> Option<(ColorImage, [u32; 2])> {
    None
}

impl Previewer {
    pub fn new(ctx: egui::Context) -> Previewer {
        let q = Arc::new(Queue { jobs: Mutex::new((VecDeque::new(), 0)), cv: Condvar::new() });
        let (tx, rx) = mpsc::channel::<Done>();
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 4);
        for _ in 0..threads {
            let q = q.clone();
            let tx: Sender<Done> = tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || loop {
                let job = {
                    let mut g = q.jobs.lock().unwrap();
                    loop {
                        if let Some(j) = g.0.pop_back() {
                            break j;
                        }
                        g = q.cv.wait(g).unwrap();
                    }
                };
                let done = match job {
                    Job::Thumb(id, dev, f) => Done::Thumb(id, decode(&dev, &f, THUMB_PX, true).map(|r| r.0)),
                    Job::Large(id, dev, f) => Done::Large(id, decode(&dev, &f, LARGE_PX, false)),
                    Job::Verify(id, dev, f) => Done::Verify(id, recover::deep_verify(&dev, &f)),
                };
                if tx.send(done).is_err() {
                    return;
                }
                ctx.request_repaint();
            });
        }
        Previewer { ctx, q, rx, thumbs: HashMap::new(), order: VecDeque::new(), large: None, verified: HashMap::new() }
    }

    pub fn reset(&mut self) {
        self.q.jobs.lock().unwrap().0.clear();
        self.thumbs.clear();
        self.order.clear();
        self.large = None;
        self.verified.clear();
    }

    fn push(&self, job: Job, front: bool) {
        let mut g = self.q.jobs.lock().unwrap();
        // Newest requests are served first (what's on screen now); stale ones are dropped.
        if front {
            g.0.push_back(job);
        } else {
            g.0.push_front(job);
        }
        while g.0.len() > 160 {
            g.0.pop_front();
        }
        self.q.cv.notify_one();
    }

    pub fn pump(&mut self) {
        while let Ok(d) = self.rx.try_recv() {
            match d {
                Done::Thumb(id, r) => {
                    let slot = match r {
                        Ok(img) => {
                            let dims = [img.size[0] as u32, img.size[1] as u32];
                            Slot::Ready(self.ctx.load_texture(format!("t{}", id), img, TextureOptions::LINEAR), dims)
                        }
                        Err(e) => Slot::Failed(e),
                    };
                    if self.thumbs.contains_key(&id) {
                        self.thumbs.insert(id, slot);
                    }
                }
                Done::Large(id, r) => {
                    if self.large.as_ref().is_some_and(|l| l.0 == id) {
                        let slot = match r {
                            Ok((img, dims)) => Slot::Ready(self.ctx.load_texture(format!("l{}", id), img, TextureOptions::LINEAR), dims),
                            Err(e) => Slot::Failed(e),
                        };
                        self.large = Some((id, slot));
                    }
                }
                Done::Verify(id, v) => {
                    self.verified.insert(id, Some(v));
                }
            }
        }
    }

    /// Thumbnail for a grid tile; queues decoding on first request.
    pub fn thumb(&mut self, dev: &Dev, f: &FoundFile) -> &Slot {
        if !self.thumbs.contains_key(&f.id) {
            self.thumbs.insert(f.id, Slot::Loading);
            self.order.push_back(f.id);
            if self.order.len() > THUMB_CACHE {
                if let Some(old) = self.order.pop_front() {
                    self.thumbs.remove(&old);
                }
            }
            self.push(Job::Thumb(f.id, dev.clone(), f.clone()), true);
        }
        &self.thumbs[&f.id]
    }

    pub fn request_large(&mut self, dev: &Dev, f: &FoundFile) {
        if self.large.as_ref().is_some_and(|l| l.0 == f.id) {
            return;
        }
        self.large = Some((f.id, Slot::Loading));
        self.push(Job::Large(f.id, dev.clone(), f.clone()), true);
    }

    pub fn request_verify(&mut self, dev: &Dev, f: &FoundFile) {
        if self.verified.contains_key(&f.id) {
            return;
        }
        self.verified.insert(f.id, None);
        self.push(Job::Verify(f.id, dev.clone(), f.clone()), true);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn decode_corpus_images() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/work/corpus");
        for name in ["progressive.jpg", "IMG_1000.jpg", "screenshot.png", "anim.gif", "wallpaper.webp", "scan.tif", "picture.bmp"] {
            let Ok(b) = std::fs::read(dir.join(name)) else { continue };
            let t = std::time::Instant::now();
            let r = image::load_from_memory(&b);
            println!("{name}: {:?} in {:?}", r.as_ref().map(|i| (i.width(), i.height())).map_err(|e| e.to_string()), t.elapsed());
            assert!(r.is_ok(), "{name}");
        }
    }
}

#[cfg(test)]
mod tests_img {
    #[test]
    fn thumbs_from_fat32_image() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/work/fat32.img");
        let Ok(dev) = relume_core::open_path(&p) else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        relume_core::scan::run_scan(dev.clone(), Default::default(), tx, Default::default());
        for ev in rx {
            if let relume_core::scan::ScanEvent::Found(fs) = ev {
                for f in fs.iter().filter(|f| f.format == relume_core::Format::Jpeg) {
                    let t = std::time::Instant::now();
                    let r = super::decode(&dev, f, 220, true);
                    println!("{} {:?} {:?}", f.name, r.as_ref().map(|x| x.1).map_err(|e| e.clone()), t.elapsed());
                }
            }
        }
    }
}
