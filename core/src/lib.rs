//! Relume recovery engine.
//!
//! * `fs`       — quick scan: deleted entries from NTFS / FAT / exFAT metadata
//! * `carve`    — deep scan: signature carving with structural validation
//! * `formats`  — 31 image & video formats: length, integrity, metadata
//! * `scan`     — orchestration + live event stream
//! * `recover`  — writing files out, previews

pub mod carve;
pub mod device;
pub mod formats;
pub mod fs;
pub mod model;
pub mod partition;
pub mod platform;
pub mod recover;
pub mod scan;
pub mod util;

pub use device::{open_path, Dev};
pub use formats::{Category, Format};
pub use model::{FileState, FoundFile, Health, Origin, ResultSet};
