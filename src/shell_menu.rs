//! The optional item "Convert with HeMiPix" in the File Explorer menu of HEIC / HEIF photos and of folders
//! (the classic menu; in Windows 11 under "Show more options"). Only in the registry of the user
//! (HKCU\Software\Classes), so no administrator rights are needed; off by default, a switch in the window.
//! Several selected photos start HeMiPix several times – the others hand their photos to the first window.

use crate::i18n::tr;
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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

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
    unsafe {
        let mut hkey: HKEY = null_mut();
        let r = RegCreateKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, null(), 0, KEY_WRITE, null(), &mut hkey, null_mut());
        if r != ERROR_SUCCESS {
            return Err(format!("{} ({r}).", tr("Writing to the registry failed", "Zápis do registru se nepodařil – chyba")));
        }
        let r = RegSetValueExW(hkey, n.as_ref().map_or(null(), |n| n.as_ptr()), 0, REG_SZ, v.as_ptr() as *const u8, (v.len() * 2) as u32);
        RegCloseKey(hkey);
        if r != ERROR_SUCCESS {
            return Err(format!("{} ({r}).", tr("Writing to the registry failed", "Zápis do registru se nepodařil – chyba")));
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
            let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
            if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
                return Err(format!("{} ({r}).", tr("The item could not be removed", "Položku se nepodařilo odebrat – chyba")));
            }
            let _ = unsafe { windows_sys::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, k.as_ptr()) };
        }
    }
    Ok(())
}

/// The item is on but points to another place (HeMiPix was moved) or has the text of the other language: write it again.
pub fn repair() {
    let Some(exe) = exe() else { return };
    if is_on()
        && (read(&format!(r"{}\command", PHOTOS[0]), None).as_deref() != Some(format!("\"{exe}\" \"%1\"").as_str())
            || read(PHOTOS[0], None).as_deref() != Some(text(false)))
    {
        let _ = set(true);
    }
}
