//! Relume command line. Run with arguments for scripting, or with none (double-click on
//! Windows) for a guided, step-by-step recovery.

mod console;

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use relume_core::formats::Category;
use relume_core::model::{FileState, FoundFile, Health, ResultSet};
use relume_core::platform::{self, DriveInfo, DriveKind};
use relume_core::recover::{self, RecoverOptions};
use relume_core::scan::{self, FolderFilter, ScanEvent, ScanRequest};
use relume_core::util::{format_date, format_duration, format_size};

use console::{bold, dim, green, red, yellow};

const USAGE: &str = "Relume CLI: photo and video recovery

USAGE
  relume-cli                      guided mode (asks what to do)
  relume-cli list                 show drives
  relume-cli scan <SOURCE> [options]

SOURCE
  E:  or  \\\\.\\E:               a drive letter
  \\\\.\\PhysicalDrive1            a whole disk (lost or formatted partitions)
  D:\\backup\\card.img             a disk image file

OPTIONS
  --quick | --deep        only the file record scan / only the raw sector scan
  --images | --videos     limit what to look for (default: both)
  --folder <PATH>         only files from this folder, e.g. \\Users\\me\\Pictures
  --recycle-bin           only files deleted through the Recycle Bin
  --all-space             also search space used by existing files (slower)
  --existing              include files that were never deleted
  --out <DIR>             recover everything found into DIR
  --json <FILE>           save the results list as JSON
  -v                      list every file";

fn main() {
    console::init();
    let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--pause").collect();
    let pause = std::env::args().any(|a| a == "--pause") || console::own_window();
    let code = match args.first().map(|s| s.as_str()) {
        None => guided(),
        Some("list") => {
            list(&platform::list_drives());
            0
        }
        Some("scan") if args.len() >= 2 => run_scan_cmd(&args),
        Some("-h" | "--help" | "help") => {
            println!("{}", USAGE);
            0
        }
        _ => {
            eprintln!("{}", USAGE);
            2
        }
    };
    if pause {
        console::wait_for_enter();
    }
    std::process::exit(code);
}

/// `E:` or `e` -> `\\.\E:` on Windows.
fn normalize_source(s: &str) -> String {
    let t = s.trim().trim_end_matches('\\');
    let b = t.as_bytes();
    if cfg!(windows) && (b.len() == 1 && b[0].is_ascii_alphabetic() || b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':') {
        return format!("\\\\.\\{}:", (b[0] as char).to_ascii_uppercase());
    }
    s.trim().trim_matches('"').to_string()
}

fn is_device(s: &str) -> bool {
    s.starts_with("\\\\.\\") || s.starts_with("\\\\?\\")
}

/// Raw drives need administrator rights. Offer to restart elevated (opens a new window).
fn ensure_admin(source: &str, args: &[String]) -> bool {
    if !is_device(source) || platform::is_elevated() {
        return true;
    }
    println!("{}", yellow("Reading drives needs administrator rights."));
    if console::ask_yes("Restart as administrator?", true) {
        let mut a = args.to_vec();
        a.push("--pause".into());
        if platform::relaunch_elevated_with(&a) {
            println!("Continuing in the new administrator window.");
            std::process::exit(0);
        }
        println!("{}", red("Could not restart as administrator. Right-click the terminal and choose \"Run as administrator\"."));
    }
    false
}

fn list(drives: &[DriveInfo]) {
    if drives.is_empty() {
        println!("  No drives found.");
        return;
    }
    for (i, d) in drives.iter().enumerate() {
        let tags = [d.fs.as_str(), d.bus.as_str(), if d.system { "system" } else { "" }, if d.kind == DriveKind::Disk { "whole disk" } else { "" }]
            .iter()
            .filter(|t| !t.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {:>2})  {:<34} {:>9}   {}", i + 1, d.title(), format_size(d.size), dim(&tags));
    }
}

fn guided() -> i32 {
    println!();
    println!("  {}  photo and video recovery", bold("Relume"));
    println!("  {}", dim("Scanning only reads. Nothing on the source is changed."));
    println!();
    if cfg!(windows) && !platform::is_elevated() {
        println!("{}", yellow("  Administrator rights are needed to read drives."));
        if console::ask_yes("  Restart as administrator?", true) && platform::relaunch_elevated_with(&["--pause".into()]) {
            return 0;
        }
    }
    let drives = platform::list_drives();
    println!("  {}", bold("Where were the files?"));
    list(&drives);
    println!("   i)  A disk image file");
    let pick = console::ask("  Choose", "1");
    let source = if pick.eq_ignore_ascii_case("i") {
        let p = console::ask("  Image file path", "");
        if p.is_empty() {
            return 1;
        }
        p.trim_matches('"').to_string()
    } else {
        match pick.parse::<usize>().ok().and_then(|n| drives.get(n.wrapping_sub(1))) {
            Some(d) => d.path.clone(),
            None => {
                println!("{}", red("  Not a valid choice."));
                return 1;
            }
        }
    };
    println!();
    println!("  {}", bold("Look for"));
    println!("   1)  Photos and videos\n   2)  Photos only\n   3)  Videos only");
    let what = console::ask("  Choose", "1");
    println!();
    println!("  {}", bold("Scan type"));
    println!("   1)  Complete: file records plus a sector-by-sector search (recommended)\n   2)  Quick: file records only, fastest\n   3)  Sectors only");
    let mode = console::ask("  Choose", "1");
    let mut args = vec!["scan".to_string(), source.clone()];
    match what.as_str() {
        "2" => args.push("--images".into()),
        "3" => args.push("--videos".into()),
        _ => {}
    }
    match mode.as_str() {
        "2" => args.push("--quick".into()),
        "3" => args.push("--deep".into()),
        _ => {}
    }
    println!();
    let Some((dev, files)) = scan_and_report(&source, &args) else { return 1 };
    if files.is_empty() {
        println!("  Nothing to recover.");
        return 0;
    }
    let good = files.iter().filter(|f| f.health != Health::Overwritten).count();
    println!();
    let dest = console::ask(&format!("  Recover {} files? Destination folder (empty to skip)", good), "");
    if dest.is_empty() {
        return 0;
    }
    let dest = PathBuf::from(dest.trim_matches('"'));
    if same_drive(&source, &dest) && !console::ask_yes(&red("  That folder is on the drive you are recovering from and could overwrite lost files. Continue anyway?"), false) {
        return 1;
    }
    let keep: Vec<FoundFile> = files.into_iter().filter(|f| f.health != Health::Overwritten).collect();
    do_recover(&dev, &keep, dest);
    0
}

fn same_drive(source: &str, dest: &std::path::Path) -> bool {
    let src = source.trim_start_matches("\\\\.\\").trim_start_matches("\\\\?\\");
    let d = dest.to_string_lossy();
    src.len() == 2 && d.len() >= 2 && src[..1].eq_ignore_ascii_case(&d[..1]) && d.as_bytes()[1] == b':'
}

fn run_scan_cmd(args: &[String]) -> i32 {
    let source = normalize_source(&args[1]);
    if !ensure_admin(&source, args) {
        return 1;
    }
    let opts = &args[2..];
    let has = |f: &str| opts.iter().any(|o| o == f);
    let val = |f: &str| opts.iter().position(|o| o == f).and_then(|i| opts.get(i + 1)).cloned();
    let Some((dev, files)) = scan_and_report(&source, args) else { return 1 };
    if has("-v") {
        for f in &files {
            print_file(f);
        }
    }
    if let Some(j) = val("--json") {
        match serde_json::to_string_pretty(&files).map_err(|e| e.to_string()).and_then(|s| std::fs::write(&j, s).map_err(|e| e.to_string())) {
            Ok(()) => println!("Saved {}", j),
            Err(e) => println!("{} {}", red("Could not save JSON:"), e),
        }
    }
    if let Some(out) = val("--out") {
        do_recover(&dev, &files, PathBuf::from(out));
    }
    0
}

fn print_file(f: &FoundFile) {
    let h = match f.health {
        Health::Excellent | Health::Good => green(f.health.label()),
        Health::Damaged => yellow(f.health.label()),
        _ => red(f.health.label()),
    };
    let info = f.info.summary();
    println!(
        "  {:<20} {:<11} {:<13} {:>9}  {:<16}  {}{}{}",
        h,
        f.state.label(),
        f.format.name(),
        format_size(f.size),
        f.date().map(format_date).unwrap_or_default(),
        f.full_path(),
        if info.is_empty() { String::new() } else { dim(&format!("  [{}]", info)) },
        if f.note.is_empty() { String::new() } else { dim(&format!("  ({})", f.note)) }
    );
}

/// Run the scan with a live progress line; returns the device and the files worth showing.
fn scan_and_report(source: &str, args: &[String]) -> Option<(relume_core::Dev, Vec<FoundFile>)> {
    let opts = &args[2..];
    let has = |f: &str| opts.iter().any(|o| o == f);
    let val = |f: &str| opts.iter().position(|o| o == f).and_then(|i| opts.get(i + 1)).cloned();
    let mut req = ScanRequest::default();
    if has("--quick") && !has("--deep") {
        req.deep = false;
    }
    if has("--deep") && !has("--quick") {
        req.quick = false;
    }
    if has("--images") && !has("--videos") {
        req.categories = vec![Category::Image];
    }
    if has("--videos") && !has("--images") {
        req.categories = vec![Category::Video];
    }
    if has("--all-space") {
        req.deep_free_only = false;
    }
    if let Some(f) = val("--folder") {
        req.folder = Some(FolderFilter::Path(f));
    }
    if has("--recycle-bin") {
        req.folder = Some(FolderFilter::RecycleBin);
    }
    let dev = match relume_core::open_path(&PathBuf::from(source)) {
        Ok(d) => d,
        Err(e) => {
            println!("{} {}: {}", red("Cannot open"), source, e);
            if is_device(source) && !platform::is_elevated() {
                println!("Run this from a terminal opened as administrator.");
            }
            return None;
        }
    };
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let (d2, c2) = (dev.clone(), cancel.clone());
    let t0 = Instant::now();
    let h = std::thread::spawn(move || scan::run_scan(d2, req, tx, c2));
    let mut results = ResultSet::new();
    let mut phase = String::from("Starting");
    let mut last = Instant::now() - std::time::Duration::from_secs(1);
    let mut frac = 0.0f32;
    for ev in rx {
        match ev {
            ScanEvent::Phase(p) => {
                console::clear_line();
                println!("  {}", dim(&p));
                phase = p;
            }
            ScanEvent::Warning(w) => {
                console::clear_line();
                println!("  {}", yellow(&w));
            }
            ScanEvent::Found(fs) => {
                for f in fs {
                    results.add(f);
                }
            }
            ScanEvent::Progress { fraction, .. } => frac = fraction,
            ScanEvent::Finished { secs, bad_sectors, cancelled } => {
                console::clear_line();
                let mut t = format!("  Scan finished in {}", format_duration(secs));
                if cancelled {
                    t.push_str(" (stopped)");
                }
                if bad_sectors > 0 {
                    t.push_str(&format!(", {} unreadable sectors skipped", bad_sectors));
                }
                println!("{}", t);
            }
            _ => {}
        }
        if last.elapsed().as_millis() > 200 {
            let photos = results.files.iter().filter(|f| f.state != FileState::Existing && f.category() == Category::Image).count();
            let videos = results.files.iter().filter(|f| f.state != FileState::Existing && f.category() == Category::Video).count();
            console::progress(frac, &format!("{} photos, {} videos  {}", photos, videos, dim(&phase)));
            last = Instant::now();
        }
    }
    let _ = h.join();
    let show_existing = has("--existing");
    let files: Vec<FoundFile> = results.files.into_iter().filter(|f| show_existing || f.state != FileState::Existing).collect();
    let count = |h: Health| files.iter().filter(|f| f.health == h).count();
    println!();
    println!(
        "  {}  {} found, {}",
        bold(&format!("{} files", files.len())),
        format_size(files.iter().map(|f| f.size).sum()),
        format!(
            "{} excellent, {} good, {} damaged, {} overwritten",
            green(&count(Health::Excellent).to_string()),
            green(&count(Health::Good).to_string()),
            yellow(&count(Health::Damaged).to_string()),
            red(&count(Health::Overwritten).to_string())
        )
    );
    let _ = t0;
    Some((dev, files))
}

fn do_recover(dev: &relume_core::Dev, files: &[FoundFile], dest: PathBuf) {
    let opts = RecoverOptions { dest, keep_folders: true };
    let total = files.len().max(1);
    let rep = recover::recover(dev, files, &opts, &AtomicBool::new(false), &mut |p| {
        console::progress(p.files_done as f32 / total as f32, &format!("{} / {}  {}", p.files_done, p.files_total, dim(&p.current)));
    });
    console::clear_line();
    println!("  {} {} files ({}) to {}", green("Recovered"), rep.recovered, format_size(rep.bytes), rep.dest.display());
    for (n, e) in rep.failed {
        println!("  {} {}: {}", red("Failed"), n, e);
    }
    let _ = io::stdout().flush();
}
