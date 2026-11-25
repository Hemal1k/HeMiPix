//! The two free extensions of the Microsoft Store that let Windows read HEIC: checked at the start (also the portable
//! HeMiPix), and – after the user accepts their license terms – installed: HEIF Image Extensions by winget, the free
//! HEVC Video Extensions (not in winget) by its install window of the Microsoft Store.

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
    let out = Command::new(program("powershell.exe"))
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
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

/// Installs what is missing. When winget fails, the install window of the Microsoft Store opens instead, so the user
/// always sees a way on (the dialog then waits for the extensions to appear).
pub fn install(m: Missing) {
    if m.heif {
        let ok = Command::new(program("winget.exe"))
            .args(["install", "--id", "9PMMSR1CGPWG", "--source", "msstore", "--silent", "--accept-package-agreements", "--accept-source-agreements"])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            // no winget (older Windows 10): the page of the store instead
            open("ms-windows-store://pdp/?ProductId=9PMMSR1CGPWG&mode=mini");
        }
    }
    if m.hevc {
        open("ms-windows-store://pdp/?ProductId=9N4WGH0Z6VHQ&mode=mini");
    }
}

/// The full path of a program of Windows given by its name: from System32, PowerShell from its own folder there,
/// winget from WindowsApps, explorer from the Windows folder. Never just the name: Windows would look into the folder
/// of HeMiPix.exe first (often Downloads), where any "powershell.exe" would then run at every start.
pub fn program(exe: &str) -> std::path::PathBuf {
    use std::path::PathBuf;
    let env = |name: &str, default: &str| std::env::var_os(name).map_or_else(|| PathBuf::from(default), PathBuf::from);
    let windows = env("SystemRoot", r"C:\Windows");
    let system = {
        use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
        let mut buf = [0u16; 260];
        // SAFETY: the buffer is valid for its length; the call writes at most that many characters
        let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if n > 0 && n < buf.len() { PathBuf::from(String::from_utf16_lossy(&buf[..n])) } else { windows.join("System32") }
    };
    match exe.to_ascii_lowercase().as_str() {
        "powershell.exe" => system.join(r"WindowsPowerShell\v1.0\powershell.exe"),
        "winget.exe" => env("LOCALAPPDATA", "").join(r"Microsoft\WindowsApps\winget.exe"),
        "explorer.exe" => windows.join("explorer.exe"),
        _ => system.join(exe),
    }
}

fn open(target: &str) {
    // nothing more to do when it fails: the dialog keeps waiting and can be closed
    let _ = crate::sys::open(target);
}
