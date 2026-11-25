//! The optional item "Convert with HeMiPix" in the File Explorer menu of HEIC / HEIF photos and of folders
//! (the classic menu; in Windows 11 under "Show more options"). Only in the registry of the user
//! (HKCU\Software\Classes), so no administrator rights are needed; off by default, a switch in the window.
//! Several selected photos start HeMiPix several times – the others hand their photos to the first window.

use crate::i18n::tr;
use crate::sys::wide;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW,
    RegSetValueExW,
};

/// The keys of the item: the photo types, and folders.
const PHOTOS: [&str; 3] = [
    r"Software\Classes\SystemFileAssociations\.heic\shell\HeMiPix",
    r"Software\Classes\SystemFileAssociations\.heif\shell\HeMiPix",
    r"Software\Classes\SystemFileAssociations\.hif\shell\HeMiPix",
];
const FOLDER: &str = r"Software\Classes\Directory\shell\HeMiPix";

fn text(folder: bool) -> &'static str {
    if folder {
        tr("Convert photos with HeMiPix", "Převést fotky v HeMiPixu")
    } else {
        tr("Convert with HeMiPix", "Převést v HeMiPixu")
    }
}

fn exe() -> Option<String> {
    Some(std::env::current_exe().ok()?.display().to_string())
}

fn read(key: &str, name: Option<&str>) -> Option<String> {
    let k = wide(key);
    let n = name.map(wide);
    let mut buf = vec![0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: `k` and `n` (or null for the default value) are NUL-terminated UTF-16; `size` is the size of `buf` in bytes.
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ref().map_or(null(), |n| n.as_ptr()), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as _, &mut size)
    };
    (r == ERROR_SUCCESS).then(|| {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    })
}

fn write(key: &str, name: Option<&str>, value: &str) -> Result<(), String> {
    let k = wide(key);
    let n = name.map(wide);
    let v = wide(value);
    // SAFETY: the strings are NUL-terminated UTF-16 that live for the calls; `v.len() * 2` is the size of the value in
    // bytes (with its NUL, as REG_SZ wants); the opened key is closed before leaving.
    unsafe {
        let mut hkey: HKEY = null_mut();
        let r = RegCreateKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, null(), 0, KEY_WRITE, null(), &mut hkey, null_mut());
        if r != ERROR_SUCCESS {
            return Err(tr("Writing to the registry failed (error {r}).", "Zápis do registru se nepodařil (chyba {r}).").replace("{r}", &r.to_string()));
        }
        let r = RegSetValueExW(hkey, n.as_ref().map_or(null(), |n| n.as_ptr()), 0, REG_SZ, v.as_ptr() as *const u8, (v.len() * 2) as u32);
        RegCloseKey(hkey);
        if r != ERROR_SUCCESS {
            return Err(tr("Writing to the registry failed (error {r}).", "Zápis do registru se nepodařil (chyba {r}).").replace("{r}", &r.to_string()));
        }
    }
    Ok(())
}

pub fn is_on() -> bool {
    read(&format!(r"{}\command", PHOTOS[0]), None).is_some()
}

pub fn set(on: bool) -> Result<(), String> {
    if on {
        let exe = exe().ok_or(tr("Could not find out where HeMiPix is.", "Nepodařilo se zjistit, kde je HeMiPix."))?;
        for (key, folder) in PHOTOS.iter().map(|k| (*k, false)).chain([(FOLDER, true)]) {
            write(key, None, text(folder))?;
            write(key, Some("Icon"), &format!("\"{exe}\",0"))?;
            // all selected photos in one go (not only the first 15)
            write(key, Some("MultiSelectModel"), "Player")?;
            write(&format!(r"{key}\command"), None, &format!("\"{exe}\" \"%1\""))?;
        }
    } else {
        for key in PHOTOS.iter().copied().chain([FOLDER]) {
            let k = wide(key);
            // SAFETY: `k` is NUL-terminated UTF-16; RegDeleteTreeW with a subkey name removes the key and all below it.
            let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
            if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
                return Err(tr("The item could not be removed (error {r}).", "Položku se nepodařilo odebrat (chyba {r}).").replace("{r}", &r.to_string()));
            }
        }
    }
    Ok(())
}

/// The exe the item starts (from its command `"<exe>" "%1"`).
fn target(command: &str) -> Option<&str> {
    command.strip_prefix('"')?.split_once('"').map(|(exe, _)| exe)
}

/// The installer after an installation or a move (`--repair`): the item, when it is on, points to this exe again and
/// has the text of the language now.
pub fn repair() {
    let Some(exe) = exe() else { return };
    if is_on()
        && (read(&format!(r"{}\command", PHOTOS[0]), None).as_deref() != Some(format!("\"{exe}\" \"%1\"").as_str())
            || read(PHOTOS[0], None).as_deref() != Some(text(false)))
    {
        // nobody to tell (the installer runs it without a window); the switch in the window shows the state
        let _ = set(true);
    }
}

/// At every start: only an item that no longer works (its exe is gone) is pointed to this exe, and the text follows the
/// language. An item of the installed HeMiPix is never taken over by a portable copy started from Downloads.
pub fn repair_if_broken() {
    let Some(exe) = exe() else { return };
    if !is_on() {
        return;
    }
    let command = read(&format!(r"{}\command", PHOTOS[0]), None).unwrap_or_default();
    let broken = target(&command).is_none_or(|t| !std::path::Path::new(t).is_file());
    let mine = target(&command).is_some_and(|t| t.eq_ignore_ascii_case(&exe));
    if broken || (mine && read(PHOTOS[0], None).as_deref() != Some(text(false))) {
        // nobody to tell before the window opens; the switch in the window shows the state
        let _ = set(true);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn target_of_the_command() {
        assert_eq!(super::target(r#""C:\Program Files\HeMi\HeMiPix\HeMiPix.exe" "%1""#), Some(r"C:\Program Files\HeMi\HeMiPix\HeMiPix.exe"));
        assert_eq!(super::target("HeMiPix.exe %1"), None);
        assert_eq!(super::target(""), None);
    }
}
