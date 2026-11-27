//! Small helpers for the Windows API that several parts of HeMiPix share.

/// The text as a NUL-terminated UTF-16 string for the wide (`…W`) functions of Windows.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Opens a web address, a file or a `ms-windows-store:` link by the default program (ShellExecute "open").
/// False when Windows could not open it.
pub fn open(target: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (verb, t) = (wide("open"), wide(target));
    // SAFETY: `verb` and `t` are NUL-terminated UTF-16 that live for the call; the other pointers may be null.
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), t.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    // ShellExecute returns a value greater than 32 on success
    r as isize > 32
}

/// Windows 11 (build 22000 and newer): its new context menu shows the item only under "Show more options".
pub fn windows_11() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let k = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let n = wide("CurrentBuildNumber");
    let mut buf = [0u16; 32];
    let mut size = std::mem::size_of_val(&buf) as u32;
    // SAFETY: the names are NUL-terminated and live for the call; `size` is the size of `buf` in bytes.
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, k.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size) };
    if r != windows_sys::Win32::Foundation::ERROR_SUCCESS {
        return false;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().parse::<u32>().is_ok_and(|b| b >= 22000)
}

#[cfg(test)]
mod tests {
    #[test]
    fn wide_ends_with_nul() {
        assert_eq!(super::wide("Ab"), vec![u16::from(b'A'), u16::from(b'b'), 0]);
    }
}
