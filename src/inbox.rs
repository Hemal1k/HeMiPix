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
    let name = crate::sys::wide("Local\\HeMiPix.Window");
    // SAFETY: `name` is NUL-terminated UTF-16; the handle is deliberately never closed (the mutex marks this process).
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        !handle.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// Leaves the photos for the running window and brings it to the front.
pub fn send(paths: &[PathBuf]) {
    if !paths.is_empty() {
        // this start has no window to report a failure in: when the inbox cannot be written, its photos are not
        // handed over (the window still comes to the front, and they can be dropped on it)
        let _ = std::fs::create_dir_all(dir());
        let text: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        let name = format!("{}-{}.txt", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        // written under another name first, so the window never reads half a list
        let tmp = dir().join(format!("{name}.part"));
        if std::fs::write(&tmp, text.join("\n")).is_ok() {
            let _ = std::fs::rename(&tmp, dir().join(name));
        }
    }
    let title = crate::sys::wide("HeMiPix");
    // SAFETY: `title` is NUL-terminated UTF-16; the window functions only get the handle FindWindowW returned (checked
    // for null), and a window that closed meanwhile only makes them fail.
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
        // a list that cannot be removed would be read again; its photos are then only known already (not added twice)
        let _ = std::fs::remove_file(&f);
    }
    paths
}
