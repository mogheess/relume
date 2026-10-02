//! Windows: drive enumeration via volume APIs and storage IOCTLs, UAC elevation, raw access.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDiskFreeSpaceW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
};
use windows_sys::Win32::System::Diagnostics::Debug::SetErrorMode;
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};

use super::{DriveInfo, DriveKind, Location};

const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;
const IOCTL_DISK_GET_DRIVE_GEOMETRY_EX: u32 = 0x0007_00A0;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_STORAGE_GET_DEVICE_NUMBER: u32 = 0x002D_1080;
const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
const FSCTL_ALLOW_EXTENDED_DASD_IO: u32 = 0x0009_0083;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(b: &[u16]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..end])
}

fn ioctl(f: &File, code: u32, input: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            f.as_raw_handle() as _,
            code,
            if input.is_empty() { null() } else { input.as_ptr() as *const c_void },
            input.len() as u32,
            if out.is_empty() { null_mut() } else { out.as_mut_ptr() as *mut c_void },
            out.len() as u32,
            &mut ret,
            null_mut(),
        )
    };
    (ok != 0).then_some(ret as usize)
}

/// Open a device for IOCTL queries: read access when allowed (needed for the size query),
/// otherwise a no-access handle, which still answers the identity/topology queries.
fn open_query(path: &str) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(path)
        .or_else(|_| OpenOptions::new().access_mode(0).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE).open(path))
}

fn length(f: &File) -> Option<u64> {
    let mut b = [0u8; 8];
    ioctl(f, IOCTL_DISK_GET_LENGTH_INFO, &[], &mut b)?;
    Some(u64::from_le_bytes(b))
}

fn geometry_size(f: &File) -> Option<u64> {
    let mut b = [0u8; 256];
    let n = ioctl(f, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], &mut b)?;
    (n >= 32).then(|| u64::from_le_bytes(b[24..32].try_into().unwrap()))
}

fn sector_size(f: &File) -> Option<u32> {
    let mut b = [0u8; 256];
    let n = ioctl(f, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], &mut b)?;
    let s = (n >= 24).then(|| u32::from_le_bytes(b[20..24].try_into().unwrap()))?;
    (s.is_power_of_two() && (512..=4096).contains(&s)).then_some(s)
}

/// Bytes per sector as the file system reports it (fallback for volume handles).
fn fs_sector_size(root: &str) -> Option<u32> {
    let r = wide(root);
    let (mut spc, mut bps, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
    let ok = unsafe { GetDiskFreeSpaceW(r.as_ptr(), &mut spc, &mut bps, &mut free, &mut total) } != 0;
    (ok && bps.is_power_of_two() && (512..=4096).contains(&bps)).then_some(bps)
}

/// `\\?\Volume{GUID}\` for a mount root such as `E:\` or `C:\mnt\usb\`.
fn volume_guid(root: &str) -> Option<String> {
    let w = wide(root);
    let mut buf = [0u16; 64];
    let ok = unsafe { GetVolumeNameForVolumeMountPointW(w.as_ptr(), buf.as_mut_ptr(), 64) } != 0;
    ok.then(|| from_wide(&buf))
}

fn bus_name(t: u32) -> &'static str {
    match t {
        1 => "SCSI",
        2 => "ATAPI",
        3 => "ATA",
        4 => "FireWire",
        7 => "USB",
        8 => "RAID",
        10 => "SAS",
        11 => "SATA",
        12 => "SD",
        13 => "MMC",
        14 | 15 => "Virtual",
        16 => "Storage Spaces",
        17 => "NVMe",
        19 => "UFS",
        _ => "",
    }
}

/// (model, bus, removable)
fn storage_props(f: &File) -> (String, String, bool) {
    let q = [0u8; 12]; // StorageDeviceProperty, PropertyStandardQuery
    let mut b = vec![0u8; 1024];
    let Some(n) = ioctl(f, IOCTL_STORAGE_QUERY_PROPERTY, &q, &mut b) else { return (String::new(), String::new(), false) };
    if n < 36 {
        return (String::new(), String::new(), false);
    }
    let rd = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let cstr = |o: usize| -> String {
        if o == 0 || o >= n {
            return String::new();
        }
        let end = b[o..n].iter().position(|&c| c == 0).map(|e| o + e).unwrap_or(n);
        String::from_utf8_lossy(&b[o..end]).trim().to_string()
    };
    let vendor = cstr(rd(12));
    let product = cstr(rd(16));
    let model = format!("{} {}", vendor, product).trim().to_string();
    (model, bus_name(rd(28) as u32).to_string(), b[10] != 0)
}

fn device_number(f: &File) -> Option<u32> {
    let mut b = [0u8; 12];
    ioctl(f, IOCTL_STORAGE_GET_DEVICE_NUMBER, &[], &mut b)?;
    Some(u32::from_le_bytes(b[4..8].try_into().unwrap()))
}

fn volume_disk(f: &File) -> Option<u32> {
    let mut b = [0u8; 256];
    let n = ioctl(f, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[], &mut b)?;
    if n < 24 || u32::from_le_bytes(b[0..4].try_into().unwrap()) == 0 {
        return None;
    }
    Some(u32::from_le_bytes(b[8..12].try_into().unwrap()))
}

pub fn list_drives() -> Vec<DriveInfo> {
    // No "There is no disk in the drive" popups for empty card-reader slots.
    unsafe { SetErrorMode(0x0001 | 0x8000) };
    let mut out = Vec::new();
    let sysdrive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_ascii_uppercase();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = wide(&format!("{}:\\", letter));
        let dtype = unsafe { GetDriveTypeW(root.as_ptr()) };
        // 2 removable, 3 fixed, 6 ramdisk; skip remote (4) and optical (5).
        if !matches!(dtype, 2 | 3 | 6) {
            continue;
        }
        let mut label = [0u16; 261];
        let mut fsname = [0u16; 261];
        let ok = unsafe {
            GetVolumeInformationW(root.as_ptr(), label.as_mut_ptr(), 261, null_mut(), null_mut(), null_mut(), fsname.as_mut_ptr(), 261)
        };
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        let has_free = unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) } != 0;
        let path = format!("\\\\.\\{}:", letter);
        let (mut size, mut model, mut bus, mut disk) = (total, String::new(), String::new(), None);
        if let Ok(f) = open_query(&path) {
            if let Some(l) = length(&f) {
                size = l;
            }
            let p = storage_props(&f);
            model = p.0;
            bus = p.1;
            disk = volume_disk(&f).or_else(|| device_number(&f));
        }
        if size == 0 {
            continue; // empty card-reader slot / no media
        }
        out.push(DriveInfo {
            path,
            kind: DriveKind::Volume,
            letter: Some(letter),
            label: if ok != 0 { from_wide(&label) } else { String::new() },
            fs: if ok != 0 { from_wide(&fsname) } else { "RAW".into() },
            size,
            free: has_free.then_some(free),
            removable: dtype == 2 || bus == "USB" || bus == "SD" || bus == "MMC",
            bus,
            model,
            disk_number: disk,
            system: format!("{}:", letter) == sysdrive,
            volume_id: volume_guid(&format!("{}:\\", letter)),
        });
    }
    for n in 0..32u32 {
        let path = format!("\\\\.\\PhysicalDrive{}", n);
        let Ok(f) = open_query(&path) else { continue };
        let size = length(&f).or_else(|| geometry_size(&f)).unwrap_or(0);
        if size == 0 {
            continue;
        }
        let (model, bus, removable) = storage_props(&f);
        let system = out.iter().any(|d| d.system && d.disk_number == Some(n));
        out.push(DriveInfo {
            path,
            kind: DriveKind::Disk,
            letter: None,
            label: String::new(),
            fs: String::new(),
            size,
            free: None,
            removable: removable || bus == "USB" || bus == "SD" || bus == "MMC",
            bus,
            model,
            disk_number: Some(n),
            system,
            volume_id: None,
        });
    }
    out
}

/// Open a raw disk/volume for reading: (file, size, sector size).
pub fn open_raw(path: &str) -> io::Result<(File, u64, u32)> {
    let f = OpenOptions::new().read(true).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE).open(path)?;
    if path.ends_with(':') || path.contains("Volume{") {
        // Let reads reach the very end of the volume (past the file system's own limit).
        let _ = ioctl(&f, FSCTL_ALLOW_EXTENDED_DASD_IO, &[], &mut []);
    }
    let len = length(&f).ok_or_else(|| io::Error::new(io::ErrorKind::Other, "cannot determine device size (no media?)"))?;
    let letter_root = path.trim_start_matches(r"\\.\").trim_start_matches(r"\\?\");
    let ss = sector_size(&f)
        .or_else(|| (letter_root.len() == 2 && letter_root.ends_with(':')).then(|| fs_sector_size(&format!("{}\\", letter_root))).flatten())
        .unwrap_or(512);
    Ok((f, len, ss))
}

pub fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

/// Restart this program with a UAC prompt. Returns true if the new process launched.
pub fn relaunch_elevated() -> bool {
    relaunch_elevated_with(&[])
}

/// Restart this program elevated, passing `args` (quoted as needed).
pub fn relaunch_elevated_with(args: &[String]) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = wide(&exe.to_string_lossy());
    let verb = wide("runas");
    let params: Vec<String> = args
        .iter()
        .map(|a| if a.is_empty() || a.contains(' ') || a.contains('"') { format!("\"{}\"", a.replace('"', "\\\"")) } else { a.clone() })
        .collect();
    let params = wide(&params.join(" "));
    let r = unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), exe.as_ptr(), params.as_ptr(), null(), 1) };
    r as isize > 32
}

pub fn open_with_default(path: &Path) -> bool {
    let p = wide(&path.to_string_lossy());
    let verb = wide("open");
    let r = unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), p.as_ptr(), null(), null(), 1) };
    r as isize > 32
}

/// Volume and physical disk that contain `path` (handles folders used as mount points).
pub fn location_of(path: &Path) -> Location {
    let p = wide(&path.to_string_lossy());
    let mut buf = [0u16; 1024];
    if unsafe { GetVolumePathNameW(p.as_ptr(), buf.as_mut_ptr(), 1024) } == 0 {
        return Location::default();
    }
    let root = from_wide(&buf);
    let volume = volume_guid(&root);
    let disk = volume
        .as_deref()
        .and_then(|v| open_query(v.trim_end_matches('\\')).ok())
        .and_then(|f| volume_disk(&f).or_else(|| device_number(&f)));
    Location { volume, disk }
}
