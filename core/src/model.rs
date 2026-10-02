//! Shared data model: found files, their on-disk layout and health.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::formats::{Category, Format, MediaInfo};

/// A run of file data. `offset` is absolute within the scanned source; `None` = sparse (zeros).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Extent {
    pub offset: Option<u64>,
    pub len: u64,
}

impl Extent {
    pub fn at(offset: u64, len: u64) -> Extent {
        Extent { offset: Some(offset), len }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Origin {
    Ntfs,
    Fat,
    ExFat,
    /// Found by signature carving (deep scan); no original name.
    Carved,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Ntfs => "NTFS record",
            Origin::Fat => "FAT entry",
            Origin::ExFat => "exFAT entry",
            Origin::Carved => "Deep scan",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileState {
    /// Deleted, metadata still present.
    Deleted,
    /// Sitting in (or emptied from) the Recycle Bin; original name restored.
    RecycleBin,
    /// No metadata left: recovered from raw data.
    Lost,
    /// Still present on the file system.
    Existing,
}

impl FileState {
    pub fn label(self) -> &'static str {
        match self {
            FileState::Deleted => "Deleted",
            FileState::RecycleBin => "Recycle Bin",
            FileState::Lost => "Lost",
            FileState::Existing => "Existing",
        }
    }
}

/// How trustworthy the recovered bytes are. Determined by actually parsing the data on disk.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Health {
    /// Structure fully validated end to end.
    Excellent,
    /// Header verified; full structure not checked or not checkable.
    Good,
    /// Starts correctly but is truncated / partly overwritten. Usually partially viewable.
    Damaged,
    /// Data no longer matches the file type (overwritten, zeroed by TRIM, ...).
    Overwritten,
    /// Not verified yet.
    Unknown,
}

impl Health {
    pub fn label(self) -> &'static str {
        match self {
            Health::Excellent => "Excellent",
            Health::Good => "Good",
            Health::Damaged => "Damaged",
            Health::Overwritten => "Overwritten",
            Health::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FoundFile {
    pub id: u64,
    pub name: String,
    /// Folder path inside the volume ("\\Users\\me\\Pictures"), or a virtual folder for lost files.
    pub path: String,
    /// Volume / partition label the file belongs to.
    pub volume: String,
    pub format: Format,
    pub size: u64,
    pub extents: Vec<Extent>,
    pub origin: Origin,
    pub state: FileState,
    pub health: Health,
    pub note: String,
    /// Unix seconds.
    pub modified: Option<i64>,
    pub created: Option<i64>,
    pub info: MediaInfo,
    /// Append a JPEG EOI marker when writing (truncated JPEG -> still viewable).
    #[serde(default)]
    pub fix_jpeg_eoi: bool,
}

impl FoundFile {
    pub fn category(&self) -> Category {
        self.format.category()
    }
    pub fn start(&self) -> Option<u64> {
        self.extents.first().and_then(|e| e.offset)
    }
    pub fn full_path(&self) -> String {
        if self.path.is_empty() {
            self.name.clone()
        } else {
            format!("{}\\{}", self.path.trim_end_matches('\\'), self.name)
        }
    }
    /// Best date for display/sorting: capture date from metadata, else FS modified time.
    pub fn date(&self) -> Option<i64> {
        self.info.taken.or(self.modified)
    }
}

/// Collection of results with de-duplication between file-system and carved hits.
#[derive(Default, Serialize, Deserialize)]
pub struct ResultSet {
    pub files: Vec<FoundFile>,
    #[serde(skip)]
    by_start: HashMap<u64, usize>,
    #[serde(skip)]
    next_id: u64,
}

impl ResultSet {
    pub fn new() -> ResultSet {
        ResultSet::default()
    }

    pub fn rebuild_index(&mut self) {
        self.by_start.clear();
        for (i, f) in self.files.iter().enumerate() {
            if let Some(s) = f.start() {
                self.by_start.entry(s).or_insert(i);
            }
            self.next_id = self.next_id.max(f.id + 1);
        }
    }

    /// Add a file. Returns Some(index) if inserted or replaced, None if dropped as duplicate.
    pub fn add(&mut self, mut f: FoundFile) -> Option<usize> {
        if let Some(start) = f.start() {
            if let Some(&i) = self.by_start.get(&start) {
                let existing = &self.files[i];
                let new_named = f.origin != Origin::Carved;
                let old_named = existing.origin != Origin::Carved;
                if new_named && !old_named {
                    // Named FS hit supersedes the anonymous carved one; keep carved media info if richer.
                    f.id = existing.id;
                    if f.info.is_empty() {
                        f.info = existing.info.clone();
                    }
                    if f.health == Health::Unknown || f.health == Health::Good {
                        if existing.health == Health::Excellent && existing.size == f.size {
                            f.health = Health::Excellent;
                        } else if existing.health == Health::Damaged && f.extents.len() == 1 && existing.size < f.size {
                            // Full structural walk by the carver found the data broken.
                            f.health = Health::Damaged;
                            f.note = if existing.note.is_empty() { "Partially overwritten".into() } else { existing.note.clone() };
                            f.fix_jpeg_eoi |= existing.fix_jpeg_eoi;
                        }
                    }
                    self.files[i] = f;
                    return Some(i);
                }
                if !new_named {
                    // Carved duplicate of a named file: let its full structural validation
                    // confirm the named entry.
                    let e = &mut self.files[i];
                    if f.health == Health::Excellent && e.size == f.size && matches!(e.health, Health::Good | Health::Unknown) {
                        e.health = Health::Excellent;
                        if e.info.is_empty() {
                            e.info = f.info;
                        }
                        return Some(i);
                    }
                    // The carver walked the whole structure and found it broken. For a
                    // contiguous file that is authoritative over the quick checks.
                    if f.health == Health::Damaged && e.health == Health::Good && e.extents.len() == 1 && f.size < e.size {
                        e.health = Health::Damaged;
                        e.note = if f.note.is_empty() { "Partially overwritten".into() } else { f.note };
                        e.fix_jpeg_eoi |= f.fix_jpeg_eoi;
                        return Some(i);
                    }
                    return None;
                }
                // Two named entries pointing at the same data (e.g. hard link, stale record): keep both.
            }
        }
        f.id = self.next_id;
        self.next_id += 1;
        let i = self.files.len();
        if let Some(s) = f.start() {
            self.by_start.entry(s).or_insert(i);
        }
        self.files.push(f);
        Some(i)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}
