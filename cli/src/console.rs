//! Terminal helpers: UTF-8 + colour on Windows consoles, prompts, a one-line progress bar,
//! and "press Enter to close" when the program owns its console window (double-clicked).

use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static COLOR: AtomicBool = AtomicBool::new(false);

pub fn init() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::*;
        SetConsoleOutputCP(65001);
        SetConsoleCP(65001);
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(h, &mut mode) != 0 && SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0 {
            COLOR.store(true, Ordering::Relaxed);
        }
    }
    #[cfg(not(windows))]
    COLOR.store(io::stdout().is_terminal(), Ordering::Relaxed);
}

/// True when this process is the only one attached to its console, i.e. it was started by
/// double-clicking and the window will vanish the moment we exit.
pub fn own_window() -> bool {
    #[cfg(windows)]
    unsafe {
        let mut ids = [0u32; 4];
        return windows_sys::Win32::System::Console::GetConsoleProcessList(ids.as_mut_ptr(), 4) == 1;
    }
    #[cfg(not(windows))]
    false
}

fn paint(code: &str, s: &str) -> String {
    if COLOR.load(Ordering::Relaxed) { format!("\x1b[{}m{}\x1b[0m", code, s) } else { s.to_string() }
}
pub fn bold(s: &str) -> String {
    paint("1", s)
}
pub fn dim(s: &str) -> String {
    paint("90", s)
}
pub fn green(s: &str) -> String {
    paint("32", s)
}
pub fn yellow(s: &str) -> String {
    paint("33", s)
}
pub fn red(s: &str) -> String {
    paint("31", s)
}

pub fn clear_line() {
    if io::stdout().is_terminal() {
        print!("\r\x1b[2K");
        let _ = io::stdout().flush();
    }
}

pub fn progress(frac: f32, text: &str) {
    if !io::stdout().is_terminal() {
        return;
    }
    let w = 28;
    let filled = ((frac.clamp(0.0, 1.0) * w as f32) as usize).min(w);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(w - filled));
    print!("\r\x1b[2K  {} {:>3.0}%  {}", paint("35", &bar), frac * 100.0, text);
    let _ = io::stdout().flush();
}

pub fn ask(prompt: &str, default: &str) -> String {
    if default.is_empty() {
        print!("{}: ", prompt);
    } else {
        print!("{} [{}]: ", prompt, default);
    }
    let _ = io::stdout().flush();
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
    let t = line.trim();
    if t.is_empty() { default.to_string() } else { t.to_string() }
}

pub fn ask_yes(prompt: &str, default: bool) -> bool {
    let a = ask(&format!("{} {}", prompt, if default { "(Y/n)" } else { "(y/N)" }), "");
    match a.to_ascii_lowercase().as_str() {
        "" => default,
        s => s.starts_with('y'),
    }
}

pub fn wait_for_enter() {
    print!("\nPress Enter to close.");
    let _ = io::stdout().flush();
    let mut s = String::new();
    let _ = io::stdin().lock().read_line(&mut s);
}
