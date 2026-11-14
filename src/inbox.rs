//! One window for all photos: File Explorer starts HeMiPix once for every selected photo; the later ones leave
//! their photos in an inbox folder (%TEMP%\HeMiPix-inbox) for the first window, bring it to the front and end.

use std::path::PathBuf;
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow};

fn dir() -> PathBuf {
    std::env::temp_dir().join("HeMiPix-inbox")
}

/// True when this is the first HeMiPix of the user (the mutex stays for the life of the process).
pub fn first() -> bool {
    let name: Vec<u16> = "Local\\HeMiPix.Window".encode_utf16().chain([0]).collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        !handle.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// Leaves the photos for the running window and brings it to the front.
pub fn send(paths: &[PathBuf]) {
    if !paths.is_empty() {
        let _ = std::fs::create_dir_all(dir());
        let text: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        let name = format!("{}-{}.txt", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        // written under another name first, so the window never reads half a list
        let tmp = dir().join(format!("{name}.part"));
        if std::fs::write(&tmp, text.join("\n")).is_ok() {
            let _ = std::fs::rename(&tmp, dir().join(name));
        }
    }
    let title: Vec<u16> = "HeMiPix".encode_utf16().chain([0]).collect();
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if !hwnd.is_null() {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
        }
    }
}

/// The photos other starts left since the last look (their lists are removed).
pub fn take() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir()) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "txt")).collect();
    files.sort();
    let mut paths = Vec::new();
    for f in files {
        if let Ok(text) = std::fs::read_to_string(&f) {
            paths.extend(text.lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from));
        }
        let _ = std::fs::remove_file(&f);
    }
    paths
}
