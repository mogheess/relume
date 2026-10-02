//! Small helpers: endian decoding on slices, time conversions, formatting.

#[inline]
pub fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
#[inline]
pub fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
#[inline]
pub fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
#[inline]
pub fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}
#[inline]
pub fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}
#[inline]
pub fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(b[o..o + 8].try_into().unwrap())
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn ymdhms_to_unix(y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Option<i64> {
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86400 + (h * 3600 + mi * 60 + s) as i64)
}

/// Plausible capture/modification date (rejects zeroed or garbage stamps).
pub fn plausible(t: i64) -> Option<i64> {
    // 1990-01-01 .. 2100-01-01
    (631152000..4102444800).contains(&t).then_some(t)
}

/// Windows FILETIME (100ns since 1601) to unix seconds.
pub fn filetime_to_unix(ft: u64) -> Option<i64> {
    if ft == 0 {
        return None;
    }
    plausible((ft / 10_000_000) as i64 - 11_644_473_600)
}

/// QuickTime/MP4 time (seconds since 1904) to unix seconds.
pub fn mac_to_unix(t: u64) -> Option<i64> {
    plausible(t as i64 - 2_082_844_800)
}

/// FAT/exFAT packed date + time to unix seconds (local time treated as UTC).
pub fn dos_to_unix(date: u16, time: u16) -> Option<i64> {
    if date == 0 {
        return None;
    }
    let y = 1980 + (date >> 9) as i64;
    let mo = ((date >> 5) & 0xF) as u32;
    let d = (date & 0x1F) as u32;
    let h = (time >> 11) as u32;
    let mi = ((time >> 5) & 0x3F) as u32;
    let s = ((time & 0x1F) * 2) as u32;
    ymdhms_to_unix(y, mo, d, h, mi, s).and_then(plausible)
}

/// EXIF "YYYY:MM:DD HH:MM:SS".
pub fn exif_date(s: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(s).ok()?.trim_matches(char::from(0)).trim();
    if s.len() < 19 {
        return None;
    }
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<u32>().ok();
    ymdhms_to_unix(n(0, 4)? as i64, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?).and_then(plausible)
}

pub fn format_date(t: i64) -> String {
    let (y, m, d) = civil_from_days(t.div_euclid(86400));
    let secs = t.rem_euclid(86400);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, secs / 3600, (secs / 60) % 60)
}

pub fn format_date_compact(t: i64) -> String {
    let (y, m, d) = civil_from_days(t.div_euclid(86400));
    let secs = t.rem_euclid(86400);
    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", y, m, d, secs / 3600, (secs / 60) % 60, secs % 60)
}

pub fn format_size(n: u64) -> String {
    const U: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{} B", n);
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{:.0} {}", v, U[i])
    } else if v >= 10.0 {
        format!("{:.1} {}", v, U[i])
    } else {
        format!("{:.2} {}", v, U[i])
    }
}

pub fn format_duration(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn utf16le_string(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

pub fn ascii_trim(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

/// Characters that are not allowed in Windows file names are replaced.
pub fn sanitize_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    if s.is_empty() {
        s.push('_');
    }
    let upper = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
        "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&upper.as_str()) {
        s.insert(0, '_');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(19000), (2022, 1, 08));
        assert_eq!(exif_date(b"2023:05:01 14:22:10"), Some(1682950930));
        assert_eq!(format_date(1682950930), "2023-05-01 14:22");
    }
}
