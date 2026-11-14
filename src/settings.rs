//! What HeMiPix remembers between runs, in %AppData%\HeMiPix\settings.ini (lines "key=value").

use crate::convert::{Format, Options};
use std::path::PathBuf;

/// Where the JPGs go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Output {
    /// Next to each photo.
    Same,
    /// Into a folder named by the format ("JPG", "PNG" …) next to each photo.
    Subfolder,
    /// All into one chosen folder.
    Folder,
}

/// When a JPG of the name exists already.
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
        }
    }
}

fn file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("HeMiPix").join("settings.ini"))
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
                "Quality" => s.quality = v.parse::<u8>().unwrap_or(85).clamp(10, 100),
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
                _ => {}
            }
        }
        // a chosen folder that is gone: next to the photos again
        if s.output == Output::Folder && s.folder.as_ref().is_none_or(|f| !f.is_dir()) {
            s.output = Output::Same;
        }
        s
    }

    pub fn save(&self) {
        let Some(f) = file() else { return };
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
            "Format={}\nQuality={}\nOutput={}\nFolder={}\nExisting={}\nKeepExif={}\nRemoveGps={}\n",
            self.format.key(),
            self.quality,
            output,
            self.folder.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            existing,
            b(self.keep_exif),
            b(self.remove_gps),
        );
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(f, text);
    }
}
