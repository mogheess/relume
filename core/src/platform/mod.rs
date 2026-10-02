//! OS integration: listing drives, privilege checks, raw device access.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DriveKind {
    /// A mounted volume (drive letter).
    Volume,
    /// A whole physical disk (all partitions + unpartitioned space).
    Disk,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriveInfo {
    /// Path to open: `\\.\E:` or `\\.\PhysicalDrive1`.
    pub path: String,
    pub kind: DriveKind,
    pub letter: Option<char>,
    pub label: String,
    pub fs: String,
    pub size: u64,
    pub free: Option<u64>,
    pub removable: bool,
    /// "USB", "SD", "NVMe", "SATA", ...
    pub bus: String,
    pub model: String,
    pub disk_number: Option<u32>,
    pub system: bool,
    /// `\\?\Volume{GUID}\` for volumes (stable identity, also for mount-point folders).
    pub volume_id: Option<String>,
}

/// Where a path lives: its volume and the physical disk under it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Location {
    pub volume: Option<String>,
    pub disk: Option<u32>,
}

impl DriveInfo {
    pub fn title(&self) -> String {
        match self.kind {
            DriveKind::Volume => {
                let l = if self.label.is_empty() {
                    if self.removable { "Removable Disk".to_string() } else { "Local Disk".to_string() }
                } else {
                    self.label.clone()
                };
                format!("{} ({}:)", l, self.letter.unwrap_or('?'))
            }
            DriveKind::Disk => {
                let m = if self.model.is_empty() { "Disk".to_string() } else { self.model.clone() };
                format!("Disk {}: {}", self.disk_number.unwrap_or(0), m)
            }
        }
    }
}

#[cfg(not(windows))]
pub fn list_drives() -> Vec<DriveInfo> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    true
}

#[cfg(not(windows))]
pub fn relaunch_elevated() -> bool {
    false
}

#[cfg(not(windows))]
pub fn relaunch_elevated_with(_args: &[String]) -> bool {
    false
}

/// Volume + physical disk holding `path` (to warn about recovering onto the source).
#[cfg(not(windows))]
pub fn location_of(_path: &std::path::Path) -> Location {
    Location::default()
}

/// Open a file or folder with the system's default application.
#[cfg(not(windows))]
pub fn open_with_default(path: &std::path::Path) -> bool {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(cmd).arg(path).spawn().is_ok()
}
