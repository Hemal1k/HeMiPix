//! Languages: English by default, Czech when the display language of Windows is Czech.

use std::sync::OnceLock;
use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;

/// The language chosen in the settings: 0 = as Windows, 1 = English, 2 = Czech.
static CHOICE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set(choice: u8) {
    CHOICE.store(choice.min(2), std::sync::atomic::Ordering::Relaxed);
}

/// True when the app speaks Czech: chosen in the settings, else as the display language of Windows (primary language 0x05).
pub fn cz() -> bool {
    match CHOICE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => return false,
        2 => return true,
        _ => {}
    }
    static CZ: OnceLock<bool> = OnceLock::new();
    *CZ.get_or_init(|| {
        // Debug builds only: HEMIPIX_LANG=en / cs to test the other language on this PC
        if cfg!(debug_assertions) {
            match std::env::var("HEMIPIX_LANG").as_deref() {
                Ok("en") => return false,
                Ok("cs") => return true,
                _ => {}
            }
        }
        (unsafe { GetUserDefaultUILanguage() } & 0x3FF) == 0x05
    })
}

/// The text in the language of the app.
pub fn tr<'a>(en: &'a str, cs: &'a str) -> &'a str {
    if cz() { cs } else { en }
}

/// The form of a word for a count, by the Czech rule: `one` for 1, `few` for 2–4, `many` for 0 and 5 and more
/// ("1 fotka", "3 fotky", "5 fotek"). For English pass the plural as both `few` and `many` ("1 photo", "2 photos").
pub fn plural<'a>(n: usize, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    match n {
        1 => one,
        2..=4 => few,
        _ => many,
    }
}

