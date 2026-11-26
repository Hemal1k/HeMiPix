//! The optional log of HeMiPix (Settings → Logging, off by default): which photo was converted to which file, in what
//! format, and the errors – in %AppData%\HeMi\HeMiPix\log.txt. Never the metadata or the location of the photos.
//! Kept under 1 MB: when it would grow over, its older half goes.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

static ON: AtomicBool = AtomicBool::new(false);
/// One writer at a time (the trimming reads and rewrites the file).
static LOCK: Mutex<()> = Mutex::new(());
/// The most the log may have, in bytes.
const LIMIT: usize = 1_000_000;

/// %AppData%\HeMi\HeMiPix\log.txt
pub fn file() -> Option<PathBuf> {
    Some(crate::settings::dir()?.join("log.txt"))
}

/// Logging on or off (from the settings); the change itself is written too.
pub fn set(on: bool) {
    let was = ON.load(Ordering::Relaxed);
    if was && !on {
        write("Logging off");
    }
    ON.store(on, Ordering::Relaxed);
    if on && !was {
        write(&format!("Logging on – HeMiPix {}", env!("CARGO_PKG_VERSION")));
    }
}

/// A line with the date and time, when logging is on. Best effort: a log that cannot be written is left out silently.
pub fn write(text: &str) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let Some(f) = file() else { return };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let line = format!("{}  {}\r\n", stamp(), text.replace(['\r', '\n'], " "));
    let size = std::fs::metadata(&f).map_or(0, |m| m.len() as usize);
    if size + line.len() > LIMIT
        && let Ok(data) = std::fs::read(&f)
    {
        let _ = std::fs::write(&f, tail(&data, LIMIT / 2));
    }
    if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(&f) {
        let _ = out.write_all(line.as_bytes());
    }
}

/// The last `keep` bytes at most, from the start of a whole line.
fn tail(data: &[u8], keep: usize) -> &[u8] {
    if data.len() <= keep {
        return data;
    }
    let cut = data.len() - keep;
    if data[cut - 1] == b'\n' {
        return &data[cut..];
    }
    match data[cut..].iter().position(|&b| b == b'\n') {
        Some(n) => &data[cut + n + 1..],
        None => &[],
    }
}

/// The local date and time, "2026-11-26 14:03:05".
fn stamp() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: GetLocalTime only fills the SYSTEMTIME it is given.
    let t = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

#[cfg(test)]
mod tests {
    use super::tail;

    #[test]
    fn tail_keeps_whole_lines() {
        assert_eq!(tail(b"one\ntwo\nthree\n", 100), b"one\ntwo\nthree\n");
        assert_eq!(tail(b"one\ntwo\nthree\n", 8), b"three\n");
        assert_eq!(tail(b"one\ntwo\nthree\n", 10), b"two\nthree\n");
        assert_eq!(tail(b"abcdef", 3), b"");
    }
}
