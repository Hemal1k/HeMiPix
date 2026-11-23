//! The two free extensions of the Microsoft Store that let Windows read HEIC: checked at the start (also the portable
//! HeMiPix), and – after the user accepts their license terms – installed by their install windows of the Microsoft
//! Store (HEIF Image Extensions and the free HEVC Video Extensions).

use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const HEIF_LICENSE: &str = "https://go.microsoft.com/fwlink/?LinkId=822631";
pub const HEVC_PAGE: &str = "https://apps.microsoft.com/detail/9N4WGH0Z6VHQ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Missing {
    pub heif: bool,
    pub hevc: bool,
}

impl Missing {
    pub fn any(self) -> bool {
        self.heif || self.hevc
    }
}

/// What is missing (asked by PowerShell – about a second, so on another thread).
pub fn check() -> Missing {
    // Debug builds only: HEMIPIX_TEST_DEPS=1 shows the dialog on a PC that has the extensions
    if cfg!(debug_assertions) && std::env::var("HEMIPIX_TEST_DEPS").is_ok() {
        return Missing { heif: true, hevc: true };
    }
    let script = "$h = [bool](Get-AppxPackage -Name Microsoft.HEIFImageExtension); $v = [bool](Get-AppxPackage -Name 'Microsoft.HEVCVideoExtension*'); \"$h $v\"";
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let Ok(out) = out else { return Missing { heif: false, hevc: false } };
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    let mut parts = text.split_whitespace();
    // when PowerShell cannot tell, nothing is offered (a failed conversion still shows the note)
    let has = |p: Option<&str>| p.is_none_or(|v| v == "true");
    let (heif, hevc) = (has(parts.next()), has(parts.next()));
    Missing { heif: !heif, hevc: !hevc }
}

/// Opens the install window of the Microsoft Store for what is missing (the Store installs it in the background).
pub fn install(m: Missing) {
    if m.heif {
        open("ms-windows-store://pdp/?ProductId=9PMMSR1CGPWG&mode=mini");
    }
    if m.hevc {
        open("ms-windows-store://pdp/?ProductId=9N4WGH0Z6VHQ&mode=mini");
    }
}

pub fn open(target: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, t) = (w("open"), w(target));
    unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), t.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
}
