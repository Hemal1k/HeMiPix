//! What HeMiPix remembers between runs, in %AppData%\HeMi\HeMiPix\settings.ini (lines "key=value").

use crate::convert::{Format, Options};
use std::path::PathBuf;

/// Where the new files go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Output {
    /// Next to each photo.
    Same,
    /// Into a folder named by the format ("JPG", "PNG" …) next to each photo.
    Subfolder,
    /// All into one chosen folder.
    Folder,
}

/// When a file of the name exists already.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Existing {
    /// "photo (2).jpg"
    Number,
    Replace,
    Skip,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    pub format: Format,
    pub quality: u8,
    pub output: Output,
    pub folder: Option<PathBuf>,
    pub existing: Existing,
    pub keep_exif: bool,
    pub remove_gps: bool,
    /// The log of the conversions (Logs\log-<day>.txt next to settings.ini); off by default.
    pub logging: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            format: Format::Jpg,
            quality: 85,
            output: Output::Same,
            folder: None,
            existing: Existing::Number,
            keep_exif: true,
            remove_gps: false,
            logging: false,
        }
    }
}

/// %AppData%\HeMi\HeMiPix – the settings of all HeMi apps are under %AppData%\HeMi; the folder of the older versions
/// (%AppData%\HeMiPix) is moved there the first time.
pub fn dir() -> Option<PathBuf> {
    let roaming = PathBuf::from(std::env::var_os("APPDATA")?);
    let dir = roaming.join("HeMi").join("HeMiPix");
    let old = roaming.join("HeMiPix");
    if !dir.exists() && old.is_dir() {
        // best effort: when the move fails, the old settings are simply not taken (the defaults, saved anew)
        let _ = std::fs::create_dir_all(roaming.join("HeMi"));
        let _ = std::fs::rename(&old, &dir);
    }
    Some(dir)
}

/// %AppData%\HeMi\HeMiPix\settings.ini
fn file() -> Option<PathBuf> {
    Some(dir()?.join("settings.ini"))
}
impl Settings {
    pub fn options(&self) -> Options {
        Options { format: self.format, quality: self.quality, keep_exif: self.keep_exif, remove_gps: self.remove_gps }
    }

    pub fn load() -> Settings {
        let mut s = Settings::default();
        let Some(text) = file().and_then(|f| std::fs::read_to_string(f).ok()) else { return s };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let b = v == "1" || v.eq_ignore_ascii_case("true");
            match k {
                "Format" => s.format = Format::from_key(v),
                "Quality" => s.quality = v.parse::<u8>().unwrap_or(85).clamp(30, 100),
                "Output" => {
                    s.output = match v {
                        "subfolder" => Output::Subfolder,
                        "folder" => Output::Folder,
                        _ => Output::Same,
                    }
                }
                "Folder" => s.folder = (!v.is_empty()).then(|| PathBuf::from(v)),
                "Existing" => {
                    s.existing = match v {
                        "replace" => Existing::Replace,
                        "skip" => Existing::Skip,
                        _ => Existing::Number,
                    }
                }
                "KeepExif" => s.keep_exif = b,
                "RemoveGps" => s.remove_gps = b,
                "Logging" => s.logging = b,
                _ => {}
            }
        }
        // a chosen folder that is gone: next to the photos again
        if s.output == Output::Folder && s.folder.as_ref().is_none_or(|f| !f.is_dir()) {
            s.output = Output::Same;
        }
        s
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(f) = file() else { return Ok(()) };
        let b = |v: bool| if v { "1" } else { "0" };
        let output = match self.output {
            Output::Same => "same",
            Output::Subfolder => "subfolder",
            Output::Folder => "folder",
        };
        let existing = match self.existing {
            Existing::Number => "number",
            Existing::Replace => "replace",
            Existing::Skip => "skip",
        };
        let text = format!(
            "Format={}\nQuality={}\nOutput={}\nFolder={}\nExisting={}\nKeepExif={}\nRemoveGps={}\nLogging={}\n",
            self.format.ext(),
            self.quality,
            output,
            self.folder.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            existing,
            b(self.keep_exif),
            b(self.remove_gps),
            b(self.logging),
        );
        write_atomic(&f, &text)
    }
}

/// Writes the file through a temporary one next to it and a rename, so a crash never leaves half a file.
pub fn write_atomic(f: &std::path::Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = f.with_extension("ini.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, f).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}
