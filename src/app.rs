//! The window of HeMiPix: the photos to convert on the left, the settings on the right, the progress, the File
//! Explorer switch and the Convert button at the bottom; the crop editor in the whole window. Laid out like
//! HeMiUnlocker and the settings of HeMiDock: a large title, small semibold section headers, cards with a frame.

use crate::convert::{self, CropRect, Done, Event, Fail, Format, Job, Preview};
use crate::crop::{Editor, Ratio};
use crate::i18n::{self, tr};
use crate::settings::{Existing, Output, Settings};
use crate::theme::{self, Palette, glyph};
use crate::deps::{self, Missing};
use crate::widgets::{Btn, CARD_RADIUS, Tone, card, checkbox, dropdown, glyph_box, info_bar, progress, row_layout, section, slider, toggle, two_lines};
use eframe::egui::{self, Align, Align2, Id, LayerId, Layout, Margin, Order, RichText, Stroke, StrokeKind, vec2};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
    WindowsDisplayHandle,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// Problems and ideas: the Issues of the project on GitHub (no e-mail address in the public code).
const CONTACT: &str = "github.com/Hemal1k/HeMiPix/issues";
const RELEASES: &str = "github.com/Hemal1k/HeMiPix/releases";
const PROJECT: &str = "github.com/Hemal1k/HeMiPix";
/// The longer side of the preview in the crop editor.
const PREVIEW: u32 = 1800;

enum State {
    Waiting,
    Working,
    Done(Done),
    /// A file of the name exists and the setting says skip.
    Skipped,
    Failed(String),
}

struct Item {
    src: PathBuf,
    size: u64,
    state: State,
    crop: Option<CropRect>,
    /// The file made last time: converted again (after a new crop) it replaces it instead of a new name.
    last_out: Option<PathBuf>,
}

struct Run {
    stop: Arc<AtomicBool>,
    total: usize,
    finished: usize,
}

struct Toast {
    text: String,
    tone: Tone,
    until: Instant,
}

enum Action {
    /// The theme or the language changed.
    SetLook(crate::look::Look),
    AddFiles,
    AddFolder,
    Remove(usize),
    Clear,
    Convert,
    Stop,
    PickOutput,
    OpenOutput,
    ShowFile(PathBuf),
    ToggleMenu,
    ToggleLogging,
    OpenLog,
    OpenSettingsFolder,
    About(bool),
    Open(String),
    OpenLicences,
    /// HeMi Launcher, for the updates.
    Launcher,
    DismissToast,
    DismissCodec,
    /// The extensions of the Microsoft Store: look again and offer them, install them, not now.
    ShowDeps,
    InstallDeps,
    CloseDeps,
    /// The crop editor: open for a photo, apply (and go on to another photo), close.
    Crop(usize),
    CropApply(Option<usize>),
    CropClose,
}

pub struct HeMiPixApp {
    s: Settings,
    items: Vec<Item>,
    run: Option<Run>,
    tx: Sender<(u64, Event)>,
    rx: Receiver<(u64, Event)>,
    run_id: u64,
    preview_tx: Sender<(u64, Result<Preview, String>)>,
    preview_rx: Receiver<(u64, Result<Preview, String>)>,
    previews: u64,
    editor: Option<Editor>,
    p: Palette,
    source: theme::Source,
    theme_checked: Instant,
    inbox_checked: Instant,
    hwnd: isize,
    /// The theme and the language chosen in the settings.
    look: crate::look::Look,
    menu_on: bool,
    about: bool,
    toast: Option<Toast>,
    /// Windows could not read a photo: the Store extensions are offered.
    no_codec: bool,
    /// Where the last converted photo went (for "Open folder").
    last_out: Option<PathBuf>,
    /// Mica of Windows 11 behind the window (the background is transparent then).
    mica: bool,
    /// The extensions for HEIC: what is missing (the dialog is open while Some), accepted, installing since.
    deps: Option<Missing>,
    deps_accept: bool,
    deps_installing: bool,
    deps_tx: Sender<Missing>,
    deps_rx: Receiver<Missing>,
}

impl HeMiPixApp {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        let look = crate::look::Look::load();
        crate::i18n::set(look.language);
        let source = theme::read_source(look.theme);
        let p = theme::palette(&source);
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, &p);
        let hwnd = match cc.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(w)) => w.hwnd.get(),
            _ => 0,
        };
        let mica = theme::style_window(hwnd, &p, true);
        let (tx, rx) = channel();
        let (preview_tx, preview_rx) = channel();
        let (deps_tx, deps_rx) = channel();
        let mut app = HeMiPixApp {
            s: Settings::load(),
            items: Vec::new(),
            run: None,
            tx,
            rx,
            run_id: 0,
            preview_tx,
            preview_rx,
            previews: 0,
            editor: None,
            p,
            source,
            theme_checked: Instant::now(),
            inbox_checked: Instant::now(),
            hwnd,
            look,
            menu_on: crate::shell_menu::is_on(),
            about: false,
            toast: None,
            no_codec: false,
            last_out: None,
            mica,
            deps: None,
            deps_accept: false,
            deps_installing: false,
            deps_tx,
            deps_rx,
        };
        crate::log::set(app.s.logging);
        app.check_deps(&cc.egui_ctx);
        app.add(paths);
        app
    }

    // ------------------------------------------------------------------ the list

    /// Adds photos, and the photos of folders (with their subfolders; from folders not the files that already are of the
    /// chosen format – chosen or dropped one by one they may be, to make them smaller or to crop them).
    fn add(&mut self, paths: Vec<PathBuf>) {
        let mut known: HashSet<String> = self.items.iter().map(|i| key(&i.src)).collect();
        let mut found = Vec::new();
        for p in paths {
            if p.is_dir() {
                walk(&p, 0, self.s.format, &mut found);
            } else if convert::is_input(&p) {
                found.push(p);
            }
        }
        for f in found {
            if known.insert(key(&f)) {
                let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
                self.items.push(Item { src: f, size, state: State::Waiting, crop: None, last_out: None });
                self.about = false;
            }
        }
    }

    fn pending(&self) -> usize {
        self.items.iter().filter(|i| matches!(i.state, State::Waiting | State::Failed(_))).count()
    }

    /// Where each waiting photo goes, by the settings; photos whose new file exists are skipped when asked.
    fn plan(&mut self) -> Result<Vec<Job>, String> {
        // the originals in the list are never a target: "Replace" must not overwrite the photo of another row
        let mut taken: HashSet<String> = self.items.iter().map(|i| key(&i.src)).collect();
        let mut jobs = Vec::new();
        for (id, item) in self.items.iter_mut().enumerate() {
            if !matches!(item.state, State::Waiting | State::Failed(_)) {
                continue;
            }
            // converted before (and cropped anew since): the same file again
            if let Some(prev) = item.last_out.clone().filter(|p| !taken.contains(&key(p))) {
                taken.insert(key(&prev));
                item.state = State::Waiting;
                jobs.push(Job { id, src: item.src.clone(), dst: prev, crop: item.crop });
                continue;
            }
            let dir = match self.s.output {
                Output::Same => item.src.parent().map(Path::to_path_buf).unwrap_or_default(),
                Output::Subfolder => item.src.parent().map(|d| d.join(self.s.format.label().to_uppercase())).unwrap_or_default(),
                Output::Folder => self.s.folder.clone().ok_or(tr("Choose the folder for the new files.", "Vyber složku pro nové soubory."))?,
            };
            if !dir.is_dir() {
                std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let stem = item.src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "photo".into());
            let ext = self.s.format.ext();
            let first = dir.join(format!("{stem}.{ext}"));
            let same = key(&first) == key(&item.src);
            let free = |p: &Path, taken: &HashSet<String>| !p.exists() && !taken.contains(&key(p));
            let dst = match self.s.existing {
                Existing::Replace if !same && !taken.contains(&key(&first)) => first,
                Existing::Skip if first.exists() || taken.contains(&key(&first)) => {
                    crate::log::write(&format!("Skipped {} – {} exists", item.src.display(), first.display()));
                    item.state = State::Skipped;
                    continue;
                }
                _ if free(&first, &taken) && !same => first,
                _ => (2..).map(|n| dir.join(format!("{stem} ({n}).{ext}"))).find(|p| free(p, &taken)).unwrap(),
            };
            taken.insert(key(&dst));
            item.state = State::Waiting;
            jobs.push(Job { id, src: item.src.clone(), dst, crop: item.crop });
        }
        Ok(jobs)
    }

    fn start(&mut self, ctx: &egui::Context) {
        let jobs = match self.plan() {
            Ok(j) => j,
            Err(e) => return self.toast(e, Tone::Critical),
        };
        if jobs.is_empty() {
            return self.toast(tr("Nothing to convert – the files exist already.", "Není co převádět – soubory už existují."), Tone::Caution);
        }
        self.save_settings();
        let quality = if self.s.format.lossy() { format!(", quality {}", self.s.quality) } else { String::new() };
        crate::log::write(&format!("Converting {} photo(s) to {}{quality}", jobs.len(), self.s.format.label()));
        self.run_id += 1;
        let stop = Arc::new(AtomicBool::new(false));
        self.run = Some(Run { stop: stop.clone(), total: jobs.len(), finished: 0 });
        self.no_codec = false;
        let (tx, id) = (self.tx.clone(), self.run_id);
        let (inner_tx, inner_rx) = channel::<Event>();
        // the events of this run, marked with its number (a stopped run may still finish a photo or two)
        std::thread::spawn(move || {
            for ev in inner_rx {
                if tx.send((id, ev)).is_err() {
                    break;
                }
            }
        });
        let wake = ctx.clone();
        convert::run(jobs, self.s.options(), stop, inner_tx, move || wake.request_repaint());
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((id, ev)) = self.rx.try_recv() {
            match ev {
                Event::Started(i) => {
                    if let Some(item) = self.items.get_mut(i) {
                        item.state = State::Working;
                    }
                }
                Event::Finished(i, r) => {
                    if id == self.run_id
                        && let Some(run) = &mut self.run
                    {
                        run.finished += 1;
                    }
                    let format = self.s.format.label();
                    let Some(item) = self.items.get_mut(i) else { continue };
                    item.state = match r {
                        Ok(d) => {
                            crate::log::write(&format!(
                                "Converted {} → {} ({format}, {} → {}{})",
                                item.src.display(),
                                d.out.display(),
                                size_text(d.in_size),
                                size_text(d.out_size),
                                d.note.map(|n| format!(", {n}")).unwrap_or_default()
                            ));
                            self.last_out = Some(d.out.clone());
                            item.last_out = Some(d.out.clone());
                            State::Done(d)
                        }
                        Err(Fail::NoCodec) => {
                            crate::log::write(&format!("Failed {}: no codec – Windows cannot read this photo", item.src.display()));
                            self.no_codec = true;
                            State::Failed(tr("Windows cannot read this photo.", "Windows tuhle fotku neumí přečíst.").into())
                        }
                        Err(Fail::Msg(m)) => {
                            crate::log::write(&format!("Failed {}: {m}", item.src.display()));
                            State::Failed(m)
                        }
                    };
                }
                Event::AllDone => {
                    if id != self.run_id {
                        continue;
                    }
                    // photos a stop left out wait again
                    for item in &mut self.items {
                        if matches!(item.state, State::Working) {
                            item.state = State::Waiting;
                        }
                    }
                    let stopped = self.run.take().is_some_and(|r| r.stop.load(Ordering::Relaxed));
                    let failed = self.items.iter().filter(|i| matches!(i.state, State::Failed(_))).count();
                    let ok = self.items.iter().filter(|i| matches!(i.state, State::Done(_))).count();
                    crate::log::write(&format!("{}: {ok} converted, {failed} failed", if stopped { "Stopped" } else { "Finished" }));
                    if stopped {
                        self.toast(tr("Stopped.", "Zastaveno."), Tone::Caution);
                    } else if failed > 0 {
                        let t = tr("{ok} converted, {bad} failed.", "Převedeno {ok}, nepovedlo se {bad}.");
                        self.toast(t.replace("{ok}", &ok.to_string()).replace("{bad}", &failed.to_string()), Tone::Caution);
                    } else {
                        self.toast(tr("All done.", "Hotovo."), Tone::Success);
                    }
                }
            }
        }
        while let Ok(m) = self.deps_rx.try_recv() {
            if self.deps_installing {
                if !m.any() {
                    self.deps_installing = false;
                    self.deps = None;
                    self.no_codec = false;
                    self.toast(tr("The extensions are installed – HEIC works now.", "Rozšíření jsou nainstalovaná – HEIC teď funguje."), Tone::Success);
                }
            } else if m.any() {
                self.deps = Some(m);
            }
        }
        while let Ok((generation, r)) = self.preview_rx.try_recv() {
            let Some(ed) = &mut self.editor else { continue };
            if ed.generation != generation {
                continue;
            }
            match r {
                Ok(p) => {
                    let crop = self.items.get(ed.idx).and_then(|i| i.crop);
                    ed.loaded(ctx, p, crop);
                }
                Err(e) => ed.error = Some(e),
            }
        }
        if self.toast.as_ref().is_some_and(|t| Instant::now() > t.until) {
            self.toast = None;
        }
    }

    fn toast(&mut self, text: impl Into<String>, tone: Tone) {
        let text = text.into();
        if tone == Tone::Critical {
            crate::log::write(&format!("Error: {text}"));
        }
        self.toast = Some(Toast { text, tone, until: Instant::now() + Duration::from_secs(6) });
    }

    /// Saves the settings; when that fails, a note says so (the settings still apply until HeMiPix ends).
    fn save_settings(&mut self) {
        if let Err(e) = self.s.save() {
            self.save_failed(&e);
        }
    }

    /// A web page or a file by its default program; a note when Windows could not open it.
    fn open_target(&mut self, target: &str) {
        if !crate::sys::open(target) {
            self.toast(tr("Windows could not open it.", "Windows to nedokázal otevřít."), Tone::Critical);
        }
    }

    fn show_file(&mut self, file: &Path) {
        if let Err(e) = show_in_explorer(file) {
            let t = tr("File Explorer could not be opened: {e}", "Průzkumník souborů se nepodařilo otevřít: {e}");
            self.toast(t.replace("{e}", &e.to_string()), Tone::Critical);
        }
    }

    fn save_failed(&mut self, e: &std::io::Error) {
        let t = tr("The settings could not be saved: {e}", "Nastavení se nepodařilo uložit: {e}");
        self.toast(t.replace("{e}", &e.to_string()), Tone::Caution);
    }

    /// Asks on another thread what of the HEIC extensions is missing (the answer opens the dialog when something is).
    fn check_deps(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.deps_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(deps::check());
            ctx.request_repaint();
        });
    }

    /// The crop editor for photo `idx`; its preview is made on another thread.
    fn open_editor(&mut self, ctx: &egui::Context, idx: usize) {
        let Some(item) = self.items.get(idx) else { return };
        self.previews += 1;
        self.editor = Some(Editor::new(idx, self.previews));
        let (src, tx, generation, ctx) = (item.src.clone(), self.preview_tx.clone(), self.previews, ctx.clone());
        std::thread::spawn(move || {
            let r = convert::preview(&src, PREVIEW).map_err(|e| match e {
                Fail::NoCodec => tr("Windows cannot read this photo.", "Windows tuhle fotku neumí přečíst.").to_string(),
                Fail::Msg(m) => m,
            });
            let _ = tx.send((generation, r));
            ctx.request_repaint();
        });
    }

    /// Light / dark or the accent color changed (in Windows or in the settings of the app): the new colors at once.
    fn follow_theme(&mut self, ctx: &egui::Context) {
        if self.theme_checked.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.theme_checked = Instant::now();
        let source = theme::read_source(self.look.theme);
        if source != self.source {
            self.source = source;
            self.p = theme::palette(&source);
            theme::apply(ctx, &self.p);
            self.mica = theme::style_window(self.hwnd, &self.p, true);
        }
    }

    /// The background of the window: transparent where Windows 11 shows Mica, else the colour of the theme.
    fn backdrop(&self) -> egui::Color32 {
        if self.mica { egui::Color32::TRANSPARENT } else { self.p.bg }
    }

    fn pick_files(&self) -> Option<Vec<PathBuf>> {
        rfd::FileDialog::new()
            .set_parent(&Parent(self.hwnd))
            .set_title(tr("Choose photos", "Vyber fotky"))
            .add_filter(tr("Photos", "Fotky"), convert::INPUTS)
            .pick_files()
    }

    fn pick_folder(&self, title: &str) -> Option<PathBuf> {
        rfd::FileDialog::new().set_parent(&Parent(self.hwnd)).set_title(title).pick_folder()
    }

    // ------------------------------------------------------------------ the window

    /// The title over the whole window, the About button on the right – as in HeMiUnlocker.
    fn header(&self, ui: &mut egui::Ui, acts: &mut Vec<Action>) {
        let p = self.p;
        ui.horizontal(|ui| {
            ui.set_min_height(40.0);
            let back = self.about || self.editor.is_some();
            if back {
                let act = if self.editor.is_some() { Action::CropClose } else { Action::About(false) };
                if Btn::new("").icon(glyph::BACK).subtle().show(ui, &p).clicked() {
                    acts.push(act);
                }
                ui.add_space(4.0);
            }
            let title = if self.about {
                tr("Settings", "Nastavení")
            } else if self.editor.is_some() {
                tr("Crop", "Oříznout")
            } else {
                "HeMiPix"
            };
            ui.label(RichText::new(title).font(theme::display(28.0)).color(p.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(ed) = &self.editor {
                    // the other photos without leaving the editor (the frame is kept)
                    let n = self.items.len();
                    let next = Btn::new("").icon(glyph::NEXT).subtle().enabled(ed.idx + 1 < n).tip(tr("Next photo", "Další fotka"));
                    if next.show(ui, &p).clicked() {
                        acts.push(Action::CropApply(Some(ed.idx + 1)));
                    }
                    ui.label(RichText::new(format!("{} / {}", ed.idx + 1, n)).size(13.0).color(p.text2));
                    let prev = Btn::new("").icon(glyph::PREVIOUS).subtle().enabled(ed.idx > 0).tip(tr("Previous photo", "Předchozí fotka"));
                    if prev.show(ui, &p).clicked() {
                        acts.push(Action::CropApply(ed.idx.checked_sub(1)));
                    }
                    ui.add_space(12.0);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        if let Some(item) = self.items.get(ed.idx) {
                            let name = item.src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            ui.add_space(8.0);
                            ui.add(egui::Label::new(RichText::new(name).size(14.0).color(p.text2)).truncate());
                        }
                    });
                } else if !self.about
                    && Btn::new("").icon(glyph::SETTINGS).subtle().show(ui, &p).clicked()
                {
                    acts.push(Action::About(true));
                }
            });
        });
    }

    fn about_page(&self, ui: &mut egui::Ui, acts: &mut Vec<Action>) {
        let p = self.p;
        // what is set once (the switch was at the bottom of the window): the item in the menu of File Explorer
        // the same at the top of every HeMi app: theme and language
        section(ui, &p, tr("Appearance", "Vzhled"));
        for (title, current, items, which) in [
            (tr("Theme", "Motiv"), self.look.theme, [tr("Automatic", "Automaticky"), tr("Light", "Světlý"), tr("Dark", "Tmavý")], 0),
            (tr("Language", "Jazyk"), self.look.language, [tr("Automatic", "Automaticky"), "English", "Čeština"], 1),
        ] {
            card(ui, &p, |ui| {
                row_layout(ui, 44.0, |ui| {
                    ui.label(RichText::new(title).size(14.0).color(p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(i) = crate::widgets::dropdown(ui, &p, current as usize, &items, 220.0) {
                            let mut l = self.look;
                            if which == 0 {
                                l.theme = i as u8;
                            } else {
                                l.language = i as u8;
                            }
                            acts.push(Action::SetLook(l));
                        }
                    });
                });
            });
        }
        ui.add_space(14.0);
        section(ui, &p, tr("File Explorer", "Průzkumník souborů"));
        card(ui, &p, |ui| {
            row_layout(ui, 44.0, |ui| {
                ui.label(RichText::new(tr("Context menu", "Kontextová nabídka")).size(14.0).color(p.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(if self.menu_on { tr("On", "Zapnuto") } else { tr("Off", "Vypnuto") }).size(14.0).color(p.text));
                    if toggle(ui, &p, self.menu_on).clicked() {
                        acts.push(Action::ToggleMenu);
                    }
                });
            });
        });
        ui.add_space(14.0);
        section(ui, &p, tr("Logging", "Logování"));
        card(ui, &p, |ui| {
            row_layout(ui, 44.0, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(if self.s.logging { tr("On", "Zapnuto") } else { tr("Off", "Vypnuto") }).size(14.0).color(p.text));
                    if toggle(ui, &p, self.s.logging).clicked() {
                        acts.push(Action::ToggleLogging);
                    }
                    ui.add_space(8.0);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        let detail = tr(
                            "Conversions and errors in log.txt (at most 1 MB), no photo metadata",
                            "Převody a chyby do log.txt (nejvýš 1 MB), žádná metadata fotek",
                        );
                        two_lines(ui, &p, tr("Logging", "Logování"), detail);
                    });
                });
            });
        });
        let files = [(tr("Open log", "Otevřít log"), "log.txt"), (tr("Open settings folder", "Otevřít složku nastavení"), r"%AppData%\HeMi\HeMiPix")];
        match card(ui, &p, |ui| crate::widgets::button_row(ui, &p, &files)) {
            Some(0) => acts.push(Action::OpenLog),
            Some(_) => acts.push(Action::OpenSettingsFolder),
            None => {}
        }
        ui.add_space(14.0);
        section(ui, &p, tr("About", "O aplikaci"));
        ui.spacing_mut().item_spacing.y = 4.0;
        let value = |ui: &mut egui::Ui, label: &str, value: &str| {
            card(ui, &p, |ui| {
                row_layout(ui, 44.0, |ui| {
                    ui.label(RichText::new(label).size(14.0).color(p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(value).size(14.0).color(p.text2));
                    });
                });
            });
        };
        value(ui, tr("Author", "Autor"), "Richard Hemala");
        value(ui, tr("Version", "Verze"), env!("CARGO_PKG_VERSION"));
        // the links side by side in one card (they wrap when the window is narrow)
        let mut links: Vec<(&str, &str, Action)> = vec![
            (tr("Project on GitHub", "Projekt na GitHubu"), PROJECT, Action::Open(format!("https://{PROJECT}"))),
            (tr("What’s new", "Co je nového"), RELEASES, Action::Open(format!("https://{RELEASES}"))),
            (tr("Report a problem", "Nahlásit problém"), CONTACT, Action::Open(format!("https://{CONTACT}"))),
            (tr("Licenses", "Licence"), "", Action::OpenLicences),
        ];
        // HeMi Launcher on this PC: the updates are its job
        if crate::launcher::exe().is_some() {
            links.insert(2, ("HeMi Launcher", tr("Updates", "Aktualizace"), Action::Launcher));
        }
        ui.add_space(10.0);
        section(ui, &p, tr("Links", "Odkazy"));
        let items: Vec<(&str, &str)> = links.iter().map(|(t, tip, _)| (*t, *tip)).collect();
        if let Some(i) = card(ui, &p, |ui| crate::widgets::button_row(ui, &p, &items))
            && let Some((_, _, act)) = links.into_iter().nth(i)
        {
            acts.push(act);
        }
    }
    fn list(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Action>) {
        let p = self.p;
        let busy = self.run.is_some();
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.label(RichText::new(tr("Photos", "Fotky")).font(theme::semibold(14.0)).color(p.text));
            if !self.items.is_empty() {
                let total: u64 = self.items.iter().map(|i| i.size).sum();
                ui.label(RichText::new(format!("{} · {}", count(self.items.len()), size_text(total))).size(13.0).color(p.text2));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !self.items.is_empty()
                    && Btn::new("").icon(glyph::DELETE).subtle().enabled(!busy).tip(tr("Clear the list", "Vyprázdnit seznam")).show(ui, &p).clicked()
                {
                    acts.push(Action::Clear);
                }
                if Btn::new(tr("Folder…", "Složka…")).icon(glyph::FOLDER).enabled(!busy).show(ui, &p).clicked() {
                    acts.push(Action::AddFolder);
                }
                let add = Btn::new(tr("Add photos…", "Přidat fotky…")).icon(glyph::ADD).enabled(!busy);
                let add = if self.items.is_empty() { add.accent() } else { add };
                if add.show(ui, &p).clicked() {
                    acts.push(Action::AddFiles);
                }
            });
        });
        ui.add_space(4.0);

        if self.items.is_empty() {
            self.empty(ui);
            return;
        }
        egui::Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.card_stroke))
            .corner_radius(CARD_RADIUS)
            .inner_margin(Margin::symmetric(6, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(ui.available_height());
                egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 48.0, self.items.len(), |ui, rows| {
                    for i in rows {
                        self.row(ui, i, busy, acts);
                    }
                });
            });
    }

    fn row(&self, ui: &mut egui::Ui, i: usize, busy: bool, acts: &mut Vec<Action>) {
        let p = self.p;
        let item = &self.items[i];
        let name = item.src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let w = ui.available_width();
        ui.allocate_ui_with_layout(vec2(w, 48.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_size(vec2(w, 48.0));
            ui.add_space(4.0);
            let (g, color) = match &item.state {
                State::Waiting | State::Working => (glyph::PHOTO, p.text2),
                State::Done(_) => (glyph::COMPLETED, p.success),
                State::Skipped => (glyph::WARNING, p.caution),
                State::Failed(_) => (glyph::ERROR, p.critical),
            };
            if matches!(item.state, State::Working) {
                let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), egui::Sense::hover());
                egui::Spinner::new().size(18.0).color(p.accent_text).paint_at(ui, egui::Rect::from_center_size(r.center(), vec2(18.0, 18.0)));
            } else {
                glyph_box(ui, g, 18.0, color);
            }
            ui.add_space(4.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !busy && Btn::new("").icon(glyph::CLOSE).subtle().tip(tr("Remove from the list", "Odebrat ze seznamu")).show(ui, &p).clicked() {
                    acts.push(Action::Remove(i));
                }
                if !busy {
                    let tip = if item.crop.is_some() { tr("Cropped – change the crop", "Oříznuto – změnit ořez") } else { tr("Crop", "Oříznout") };
                    let crop = Btn::new("").icon(glyph::CROP).subtle().tip(tip);
                    let crop = if item.crop.is_some() { crop.tint(p.accent_text) } else { crop };
                    if crop.show(ui, &p).clicked() {
                        acts.push(Action::Crop(i));
                    }
                }
                if let State::Done(d) = &item.state
                    && Btn::new("").icon(glyph::OPEN_FOLDER).subtle().tip(tr("Show in File Explorer", "Zobrazit v Průzkumníkovi souborů")).show(ui, &p).clicked()
                {
                    acts.push(Action::ShowFile(d.out.clone()));
                }
                ui.add_space(6.0);
                // the result on the right
                match &item.state {
                    State::Done(d) => {
                        let less = 100.0 - d.out_size as f64 * 100.0 / d.in_size.max(1) as f64;
                        let pct = if less >= 0.5 { format!("−{}", percent(&format!("{less:.0}"))) } else { format!("+{}", percent(&format!("{:.0}", -less))) };
                        ui.label(RichText::new(pct).font(theme::semibold(13.0)).color(if less >= 0.5 { p.success } else { p.text2 }));
                        ui.label(RichText::new(format!("{} → {}", size_text(d.in_size), size_text(d.out_size))).size(13.0).color(p.text2));
                    }
                    State::Working => {
                        ui.label(RichText::new(tr("Converting…", "Převádím…")).size(13.0).color(p.accent_text));
                    }
                    State::Skipped => {
                        ui.label(RichText::new(tr("Skipped – the file exists", "Přeskočeno – soubor už existuje")).size(13.0).color(p.caution));
                    }
                    _ => {
                        ui.label(RichText::new(size_text(item.size)).size(13.0).color(p.text2));
                    }
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let cropped = if item.crop.is_some() { tr("Cropped · ", "Oříznuto · ") } else { "" };
                    let detail = match &item.state {
                        State::Done(d) => format!(
                            "→ {} · {} × {}{}",
                            d.out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                            d.width,
                            d.height,
                            d.note.map(|n| format!(" · {n}")).unwrap_or_default()
                        ),
                        State::Failed(e) => e.clone(),
                        _ => format!("{cropped}{}", item.src.parent().map(|d| d.display().to_string()).unwrap_or_default()),
                    };
                    let detail_color = if matches!(item.state, State::Failed(_)) { p.critical } else { p.text2 };
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.add(egui::Label::new(RichText::new(name).size(14.0).color(p.text)).truncate());
                        ui.add(egui::Label::new(RichText::new(detail).size(12.0).color(detail_color)).truncate());
                    });
                });
            });
        });
    }

    fn empty(&self, ui: &mut egui::Ui) {
        let p = self.p;
        let r = ui.available_rect_before_wrap();
        ui.painter().rect_stroke(r.shrink(1.0), CARD_RADIUS, Stroke::new(1.0, p.card_stroke), StrokeKind::Inside);
        ui.allocate_rect(r, egui::Sense::hover());
        let c = r.center();
        let painter = ui.painter();
        painter.text(c - vec2(0.0, 44.0), Align2::CENTER_CENTER, glyph::PHOTO, theme::icon(44.0), p.text3);
        painter.text(c + vec2(0.0, 8.0), Align2::CENTER_CENTER, tr("Drop photos or folders here", "Přetáhni sem fotky nebo složky"), theme::semibold(16.0), p.text);
        let hint = tr(
            "To JPG, PNG, WebP or TIFF – all at once.",
            "Na JPG, PNG, WebP nebo TIFF – všechny najednou.",
        );
        painter.text(c + vec2(0.0, 34.0), Align2::CENTER_CENTER, hint, theme::body(13.0), p.text2);
    }

    /// The settings on the right: quality, where to save, what to do with an existing file, metadata.
    fn settings(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Action>) {
        let p = self.p;
        let before = self.s.clone();
        let busy = self.run.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let width = ui.available_width();
            let control = 168.0;

            ui.horizontal(|ui| {
                ui.set_min_height(32.0);
                ui.label(RichText::new(tr("Result", "Výsledek")).font(theme::semibold(14.0)).color(p.text));
            });
            ui.add_space(4.0);
            card(ui, &p, |ui| {
                row_layout(ui, 36.0, |ui| {
                    two_lines(ui, &p, tr("Format", "Formát"), tr("What the photos are converted to", "Na co se fotky převedou"));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let items: Vec<&str> = Format::ALL.iter().map(|f| f.label()).collect();
                        let cur = Format::ALL.iter().position(|&f| f == self.s.format).unwrap_or(0);
                        if let Some(i) = dropdown(ui, &p, cur, &items, control) {
                            self.s.format = Format::ALL[i];
                        }
                    });
                });
            });
            card(ui, &p, |ui| {
                if self.s.format.lossy() {
                    row_layout(ui, 32.0, |ui| {
                        ui.label(RichText::new(tr("Quality", "Kvalita")).size(14.0).color(p.text));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(self.s.quality.to_string()).font(theme::semibold(14.0)).color(p.text));
                        });
                    });
                    slider(ui, &p, &mut self.s.quality, 30..=100, width - 30.0);
                    let hint = match self.s.quality {
                        0..=59 => tr("Small file, visibly worse picture", "Malý soubor, viditelně horší obraz"),
                        60..=79 => tr("Smaller file, good for sharing", "Menší soubor, dobré na sdílení"),
                        80..=90 => tr("Hard to tell from the original", "Od originálu skoro k nerozeznání"),
                        _ => tr("Best picture, bigger file", "Nejlepší obraz, větší soubor"),
                    };
                    ui.label(RichText::new(hint).size(12.0).color(p.text2));
                } else {
                    row_layout(ui, 36.0, |ui| {
                        two_lines(
                            ui,
                            &p,
                            tr("Quality", "Kvalita"),
                            tr("Lossless – every pixel stays, the files are bigger", "Bezztrátově – zůstane každý pixel, soubory jsou větší"),
                        );
                    });
                }
            });

            ui.add_space(18.0);
            section(ui, &p, tr("Saving", "Ukládání"));
            card(ui, &p, |ui| {
                row_layout(ui, 36.0, |ui| {
                    ui.label(RichText::new(tr("Save to", "Uložit")).size(14.0).color(p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let folder = self.s.format.label().to_uppercase();
                        let beside = if i18n::cz() { format!("Do složky „{folder}“") } else { format!("Folder “{folder}”") };
                        let items = [tr("Next to the photos", "Vedle fotek"), beside.as_str(), tr("A chosen folder", "Do vybrané složky")];
                        let cur = match self.s.output {
                            Output::Same => 0,
                            Output::Subfolder => 1,
                            Output::Folder => 2,
                        };
                        match dropdown(ui, &p, cur, &items, control) {
                            Some(0) => self.s.output = Output::Same,
                            Some(1) => self.s.output = Output::Subfolder,
                            Some(_) => acts.push(Action::PickOutput),
                            None => {}
                        }
                    });
                });
                if self.s.output == Output::Folder
                    && let Some(f) = &self.s.folder
                {
                    ui.add(egui::Label::new(RichText::new(f.display().to_string()).size(12.0).color(p.text2)).truncate());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if Btn::new(tr("Change…", "Změnit…")).show(ui, &p).clicked() {
                            acts.push(Action::PickOutput);
                        }
                    });
                }
            });
            card(ui, &p, |ui| {
                row_layout(ui, 36.0, |ui| {
                    ui.label(RichText::new(tr("If the file exists", "Když soubor už existuje")).size(14.0).color(p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let items = [tr("Add a number", "Přidat číslo"), tr("Replace it", "Nahradit"), tr("Skip the photo", "Fotku přeskočit")];
                        let cur = match self.s.existing {
                            Existing::Number => 0,
                            Existing::Replace => 1,
                            Existing::Skip => 2,
                        };
                        if let Some(i) = dropdown(ui, &p, cur, &items, control) {
                            self.s.existing = [Existing::Number, Existing::Replace, Existing::Skip][i];
                        }
                    });
                });
            });
            card(ui, &p, |ui| {
                row_layout(ui, 36.0, |ui| {
                    // one choice: everything, everything but the location, or nothing (TIFF gets none)
                    let detail = if self.s.format.has_exif() && self.s.keep_exif && self.s.remove_gps {
                        // only the GPS block goes: the serial numbers, the owner's name and the MakerNote stay
                        tr("Camera serial numbers stay", "Sériová čísla fotoaparátu zůstanou")
                    } else if self.s.format.has_exif() {
                        tr("Date, camera, location", "Datum, fotoaparát, poloha")
                    } else {
                        tr("Not carried into TIFF", "Do TIFFu se nepřenášejí")
                    };
                    two_lines(ui, &p, tr("Metadata", "Metadata"), detail);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let items = [tr("Keep all", "Ponechat vše"), tr("Without location", "Bez polohy"), tr("Remove", "Odstranit")];
                        let cur = match (self.s.keep_exif, self.s.remove_gps) {
                            (false, _) => 2,
                            (true, true) => 1,
                            (true, false) => 0,
                        };
                        if let Some(i) = dropdown(ui, &p, cur, &items, control) {
                            (self.s.keep_exif, self.s.remove_gps) = [(true, false), (true, true), (false, false)][i];
                        }
                    });
                });
            });
        });
        if self.s != before {
            self.save_settings();
        }
    }

    /// The bottom: notes, then the File Explorer switch on the left (as in HeMiUnlocker) and the work on the right.
    fn footer(&self, ui: &mut egui::Ui, acts: &mut Vec<Action>) {
        let p = self.p;
        if self.no_codec {
            info_bar(
                ui,
                &p,
                Tone::Caution,
                tr(
                    "Windows cannot read HEIC yet: it needs two free extensions from the Microsoft Store.",
                    "Windows zatím neumí číst HEIC: potřebuje dvě bezplatná rozšíření z Microsoft Storu.",
                ),
                None,
            );
            ui.horizontal(|ui| {
                if Btn::new(tr("Install the extensions…", "Nainstalovat rozšíření…")).icon(glyph::ADD).accent().show(ui, &p).clicked() {
                    acts.push(Action::ShowDeps);
                }
                if Btn::new(tr("Hide", "Skrýt")).subtle().show(ui, &p).clicked() {
                    acts.push(Action::DismissCodec);
                }
            });
            ui.add_space(8.0);
        }
        if let Some(t) = &self.toast {
            if info_bar(ui, &p, t.tone, &t.text, Some(())) {
                acts.push(Action::DismissToast);
            }
            ui.add_space(8.0);
        }
        // the progress, or what the last conversion saved
        if let Some(run) = &self.run {
            let t = tr("Converting {a} of {b}…", "Převádím {a} z {b}…")
                .replace("{a}", &(run.finished + 1).min(run.total).to_string())
                .replace("{b}", &run.total.to_string());
            ui.label(RichText::new(t).size(13.0).color(p.text));
            ui.add_space(4.0);
            progress(ui, &p, run.finished as f32 / run.total.max(1) as f32, ui.available_width());
            ui.add_space(10.0);
        } else {
            let done: Vec<&Done> = self.items.iter().filter_map(|i| if let State::Done(d) = &i.state { Some(d) } else { None }).collect();
            if !done.is_empty() {
                let (a, b) = (done.iter().map(|d| d.in_size).sum::<u64>(), done.iter().map(|d| d.out_size).sum::<u64>());
                let less = 100.0 - b as f64 * 100.0 / a.max(1) as f64;
                ui.horizontal(|ui| {
                    // Czech: the verb agrees with the count ("Převedena 1 fotka", "Převedeny 3 fotky", "Převedeno 5 fotek")
                    let t = tr("{n} converted · {a} → {b}", "{verb} {n} · {a} → {b}")
                        .replace("{verb}", i18n::plural(done.len(), "Převedena", "Převedeny", "Převedeno"))
                        .replace("{n}", &count(done.len()))
                        .replace("{a}", &size_text(a))
                        .replace("{b}", &size_text(b));
                    ui.label(RichText::new(t).size(13.0).color(p.text));
                    if less >= 0.5 {
                        ui.label(RichText::new(format!("(−{})", percent(&format!("{less:.0}")))).font(theme::semibold(13.0)).color(p.success));
                    }
                });
                ui.add_space(6.0);
            }
        }
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.run.is_some() {
                    if Btn::new(tr("Stop", "Zastavit")).icon(glyph::STOP).show(ui, &p).clicked() {
                        acts.push(Action::Stop);
                    }
                    return;
                }
                let pending = self.pending();
                let label = if pending > 0 { tr("Convert {n}", "Převést {n}").replace("{n}", &count_object(pending)) } else { tr("Convert", "Převést").to_string() };
                if Btn::new(&label).icon(glyph::CHECK).accent().enabled(pending > 0).show(ui, &p).clicked() {
                    acts.push(Action::Convert);
                }
                if self.last_out.is_some() && Btn::new(tr("Open folder", "Otevřít složku")).icon(glyph::OPEN_FOLDER).show(ui, &p).clicked() {
                    acts.push(Action::OpenOutput);
                }
            });
        });
    }

    /// The bottom of the crop editor: the shapes on the left, the size and the buttons on the right.
    fn crop_footer(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Action>) {
        let p = self.p;
        let Some(ed) = &mut self.editor else { return };
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.add_enabled_ui(!ed.loading() && ed.error.is_none(), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for r in Ratio::ALL {
                    let b = Btn::new(r.label());
                    let b = if ed.ratio == r { b.accent() } else { b };
                    if b.show(ui, &p).clicked() {
                        ed.set_ratio(r);
                    }
                }
                ui.add_space(4.0);
                let tip = tr("Turn the shape (landscape / portrait)", "Otočit tvar (na šířku / na výšku)");
                if Btn::new("").icon(glyph::ROTATE).enabled(ed.ratio != Ratio::Free && ed.ratio != Ratio::R1x1).tip(tip).show(ui, &p).clicked() {
                    ed.turn();
                }
                ui.add_space(12.0);
                if !ed.loading() {
                    let (w, h) = ed.pixels();
                    ui.label(RichText::new(format!("{w} × {h} px")).size(13.0).color(p.text2));
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if Btn::new(tr("Done", "Hotovo")).icon(glyph::CHECK).accent().enabled(ed.error.is_none()).show(ui, &p).clicked() {
                    acts.push(Action::CropApply(None));
                }
                if Btn::new(tr("Cancel", "Zrušit")).show(ui, &p).clicked() {
                    acts.push(Action::CropClose);
                }
                if Btn::new(tr("Whole photo", "Celá fotka")).subtle().enabled(!ed.loading()).show(ui, &p).clicked() {
                    ed.reset();
                }
            });
        });
    }

    /// "Photo extensions from Microsoft": as the page of the installer – what is missing, the links to the license terms,
    /// the acceptance, then Install.
    fn deps_dialog(&mut self, ctx: &egui::Context, acts: &mut Vec<Action>) {
        let Some(m) = self.deps else { return };
        let p = self.p;
        let frame = egui::Frame::new().fill(p.dialog).stroke(Stroke::new(1.0, p.card_stroke)).corner_radius(8).shadow(ctx.global_style().visuals.window_shadow);
        let width = 480.0;
        let modal = egui::Modal::new(Id::unique("deps")).backdrop_color(p.smoke).frame(frame).show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            egui::Frame::new().inner_margin(Margin { left: 24, right: 24, top: 22, bottom: 20 }).show(ui, |ui| {
                ui.set_width(width - 48.0);
                ui.label(RichText::new(tr("Photo extensions from Microsoft", "Rozšíření fotek od Microsoftu")).font(theme::semibold(20.0)).color(p.text));
                ui.add_space(10.0);
                let text = tr(
                    "HEIC photos are read by Windows itself, with two free extensions. Without them HeMiPix converts everything but HEIC. Missing on this PC:",
                    "Fotky HEIC čte samotný Windows pomocí dvou bezplatných rozšíření. Bez nich HeMiPix převede vše kromě HEIC. V tomto počítači chybí:",
                );
                ui.add(egui::Label::new(RichText::new(text).size(14.0).color(p.text)).wrap());
                ui.add_space(12.0);
                for (name, link, missing) in [("HEIF Image Extensions", deps::HEIF_LICENSE, m.heif), ("HEVC Video Extensions", deps::HEVC_PAGE, m.hevc)] {
                    if !missing {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(RichText::new(format!("•  {name}")).font(theme::semibold(14.0)).color(p.text));
                        if ui.add(egui::Label::new(RichText::new(tr("license terms", "licenční podmínky")).size(14.0).color(p.accent_text).underline()).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            acts.push(Action::Open(link.into()));
                        }
                    });
                    ui.add_space(6.0);
                }
                if m.heif {
                    // winget accepts the terms of its "msstore" source for the user, which send the region to Microsoft
                    let note = tr(
                        "HEIF is installed by winget from the Microsoft Store (which receives your region).",
                        "HEIF nainstaluje winget z Microsoft Storu (ten dostane tvůj region).",
                    );
                    ui.add(egui::Label::new(RichText::new(note).size(13.0).color(p.text2)).wrap());
                    ui.add_space(6.0);
                }
                ui.add_space(8.0);
                if self.deps_installing {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0).color(p.accent_text));
                        ui.add_space(8.0);
                        let wait = tr("Installing – if the Microsoft Store window opened, finish the installation there…", "Instaluji – pokud se otevřelo okno Microsoft Storu, dokonči v něm instalaci…");
                        ui.add(egui::Label::new(RichText::new(wait).size(13.0).color(p.text2)).wrap());
                    });
                } else if checkbox(ui, &p, self.deps_accept, tr("I accept the license terms and want them installed", "Souhlasím s licenčními podmínkami a chci je nainstalovat")).clicked() {
                    self.deps_accept = !self.deps_accept;
                }
            });
            egui::Frame::new().fill(p.dialog_foot).corner_radius(egui::CornerRadius { nw: 0, ne: 0, sw: 8, se: 8 }).inner_margin(Margin::same(24)).show(ui, |ui| {
                ui.set_width(width - 48.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let half = (width - 48.0 - 8.0) / 2.0;
                    let install = Btn::new(tr("Install", "Nainstalovat")).accent().width(half).enabled(self.deps_accept && !self.deps_installing);
                    if install.show(ui, &p).clicked() {
                        acts.push(Action::InstallDeps);
                    }
                    let close = if self.deps_installing { tr("Close", "Zavřít") } else { tr("Not now", "Teď ne") };
                    if Btn::new(close).width(half).show(ui, &p).clicked() {
                        acts.push(Action::CloseDeps);
                    }
                });
            });
        });
        if modal.should_close() && !self.deps_installing {
            acts.push(Action::CloseDeps);
        }
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        if !ctx.input(|i| !i.raw.hovered_files.is_empty()) || self.run.is_some() || self.editor.is_some() {
            return;
        }
        let p = self.p;
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("drop")));
        let r = ctx.content_rect().shrink(14.0);
        painter.rect_filled(r, CARD_RADIUS, p.bg.gamma_multiply(0.94));
        painter.rect_stroke(r, CARD_RADIUS, Stroke::new(2.0, p.accent), StrokeKind::Inside);
        painter.text(r.center() - vec2(0.0, 22.0), Align2::CENTER_CENTER, glyph::PHOTO, theme::icon(36.0), p.accent_text);
        painter.text(r.center() + vec2(0.0, 22.0), Align2::CENTER_CENTER, tr("Drop the photos here", "Pusť fotky sem"), theme::semibold(20.0), p.text);
    }

    fn act(&mut self, ctx: &egui::Context, acts: Vec<Action>) {
        for a in acts {
            match a {
                Action::SetLook(l) => {
                    if let Err(e) = l.save() {
                        self.save_failed(&e);
                    }
                    crate::i18n::set(l.language);
                    self.look = l;
                    // the new colours at once, not after the next look at the theme
                    self.theme_checked = Instant::now().checked_sub(Duration::from_secs(5)).unwrap_or_else(Instant::now);
                }
                Action::AddFiles => {
                    if let Some(f) = self.pick_files() {
                        self.add(f);
                    }
                }
                Action::AddFolder => {
                    if let Some(d) = self.pick_folder(tr("Choose a folder with photos", "Vyber složku s fotkami")) {
                        self.add(vec![d]);
                    }
                }
                Action::Remove(i) => {
                    if i < self.items.len() {
                        self.items.remove(i);
                    }
                }
                Action::Clear => {
                    self.items.clear();
                    self.last_out = None;
                }
                Action::Convert => self.start(ctx),
                Action::Stop => {
                    if let Some(r) = &self.run {
                        r.stop.store(true, Ordering::Relaxed);
                    }
                }
                Action::PickOutput => {
                    if let Some(d) = self.pick_folder(tr("Choose the folder for the new files", "Vyber složku pro nové soubory")) {
                        self.s.folder = Some(d);
                        self.s.output = Output::Folder;
                        self.save_settings();
                    }
                }
                Action::OpenOutput => {
                    if let Some(f) = self.last_out.clone() {
                        self.show_file(&f);
                    }
                }
                Action::ShowFile(f) => self.show_file(&f),
                Action::ToggleMenu => match crate::shell_menu::set(!self.menu_on) {
                    Ok(()) => self.menu_on = crate::shell_menu::is_on(),
                    Err(e) => self.toast(e, Tone::Critical),
                },
                Action::ToggleLogging => {
                    self.s.logging = !self.s.logging;
                    crate::log::set(self.s.logging);
                    self.save_settings();
                }
                Action::OpenLog => match crate::log::file().filter(|f| f.is_file()) {
                    Some(f) => self.open_target(&f.display().to_string()),
                    None => {
                        let t = tr("There is no log yet – turn on Logging and convert some photos.", "Log zatím není – zapni Logování a převeď nějaké fotky.");
                        self.toast(t, Tone::Caution);
                    }
                },
                Action::OpenSettingsFolder => {
                    let r = crate::settings::dir()
                        .ok_or_else(|| std::io::Error::other("%AppData%"))
                        .and_then(|d| std::fs::create_dir_all(&d).map(|()| d))
                        .and_then(|d| std::process::Command::new(crate::deps::program("explorer.exe")).arg(d).spawn().map(drop));
                    if let Err(e) = r {
                        let t = tr("File Explorer could not be opened: {e}", "Průzkumník souborů se nepodařilo otevřít: {e}");
                        self.toast(t.replace("{e}", &e.to_string()), Tone::Critical);
                    }
                }
                Action::About(on) => self.about = on,
                Action::Open(t) => self.open_target(&t),
                Action::Launcher => {
                    if let Err(e) = crate::launcher::open() {
                        let t = tr("HeMi Launcher could not be started: {e}", "HeMi Launcher se nepodařilo spustit: {e}");
                        self.toast(t.replace("{e}", &e.to_string()), Tone::Critical);
                    }
                }
                Action::OpenLicences => {
                    let file = std::env::temp_dir().join("HeMiPix-ThirdPartyNotices.txt");
                    match std::fs::write(&file, include_str!("../assets/ThirdPartyNotices.txt")) {
                        Ok(()) => self.open_target(&file.display().to_string()),
                        Err(e) => self.toast(e.to_string(), Tone::Critical),
                    }
                }
                Action::DismissToast => self.toast = None,
                Action::DismissCodec => self.no_codec = false,
                Action::ShowDeps => {
                    self.deps = Some(Missing { heif: true, hevc: true });
                    self.check_deps(ctx);
                }
                Action::InstallDeps => {
                    if let Some(m) = self.deps {
                        self.deps_installing = true;
                        let (tx, ctx) = (self.deps_tx.clone(), ctx.clone());
                        std::thread::spawn(move || {
                            deps::install(m);
                            // the Store installs in the background: looked again for two minutes
                            for _ in 0..24 {
                                let now = deps::check();
                                let _ = tx.send(now);
                                ctx.request_repaint();
                                if !now.any() {
                                    break;
                                }
                                std::thread::sleep(Duration::from_secs(5));
                            }
                        });
                    }
                }
                Action::CloseDeps => {
                    self.deps = None;
                    self.deps_installing = false;
                }
                Action::Crop(i) => self.open_editor(ctx, i),
                Action::CropApply(next) => {
                    if let Some(ed) = self.editor.take()
                        && ed.error.is_none()
                        && !ed.loading()
                        && let Some(item) = self.items.get_mut(ed.idx)
                    {
                        let crop = ed.result();
                        if item.crop != crop {
                            item.crop = crop;
                            // a converted photo cropped anew waits to be converted again (into the same file)
                            if matches!(item.state, State::Done(_) | State::Skipped) {
                                item.state = State::Waiting;
                            }
                        }
                    }
                    if let Some(n) = next {
                        self.open_editor(ctx, n);
                    }
                }
                Action::CropClose => self.editor = None,
            }
        }
    }
}

impl eframe::App for HeMiPixApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.follow_theme(&ctx);
        ctx.request_repaint_after(Duration::from_millis(500));

        // photos dropped from File Explorer, and photos other starts of HeMiPix left (File Explorer item)
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() && self.run.is_none() && self.editor.is_none() {
            self.add(dropped);
        }
        if self.inbox_checked.elapsed() >= Duration::from_millis(400) && self.run.is_none() {
            self.inbox_checked = Instant::now();
            let sent = crate::inbox::take();
            if !sent.is_empty() {
                self.add(sent);
            }
        }
        // the crop editor: Enter keeps the frame, Esc leaves it
        if self.editor.is_some() {
            let (enter, esc) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            if esc {
                self.editor = None;
            } else if enter {
                self.act(&ctx, vec![Action::CropApply(None)]);
            }
        }

        let bg = self.backdrop();
        let mut acts = Vec::new();
        egui::Panel::top("header")
            .frame(egui::Frame::new().fill(bg).inner_margin(Margin { left: 28, right: 28, top: 14, bottom: 14 }))
            .show_separator_line(false)
            .show(ui, |ui| self.header(ui, &mut acts));
        if self.editor.is_some() {
            egui::Panel::bottom("crop-footer")
                .frame(egui::Frame::new().fill(bg).inner_margin(Margin { left: 28, right: 28, top: 10, bottom: 16 }))
                .show_separator_line(false)
                .show(ui, |ui| self.crop_footer(ui, &mut acts));
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(bg).inner_margin(Margin { left: 28, right: 28, top: 0, bottom: 0 }))
                .show(ui, |ui| {
                    let p = self.p;
                    egui::Frame::new()
                        .fill(p.card)
                        .stroke(Stroke::new(1.0, p.card_stroke))
                        .corner_radius(CARD_RADIUS)
                        .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            if let Some(ed) = &mut self.editor {
                                ed.show(ui, &p);
                            }
                        });
                });
        } else if self.about {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(bg).inner_margin(Margin { left: 28, right: 28, top: 0, bottom: 4 }))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.about_page(ui, &mut acts));
                });
        } else {
            egui::Panel::bottom("footer")
                .frame(egui::Frame::new().fill(bg).inner_margin(Margin { left: 28, right: 28, top: 10, bottom: 16 }))
                .show_separator_line(false)
                .show(ui, |ui| self.footer(ui, &mut acts));
            egui::Panel::right("settings")
                .exact_size(374.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(bg).inner_margin(Margin { left: 8, right: 14, top: 0, bottom: 4 }))
                .show_separator_line(false)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        egui::Frame::new().inner_margin(Margin { left: 0, right: 14, top: 0, bottom: 8 }).show(ui, |ui| self.settings(ui, &mut acts));
                    });
                });
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(bg).inner_margin(Margin { left: 28, right: 20, top: 0, bottom: 4 }))
                .show(ui, |ui| self.list(ui, &mut acts));
        }
        self.drop_overlay(&ctx);
        self.deps_dialog(&ctx, &mut acts);
        self.act(&ctx, acts);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.backdrop().to_normalized_gamma_f32()
    }
}

// ---------------------------------------------------------------------- helpers

/// A path as a key for comparing (Windows does not mind the case).
fn key(p: &Path) -> String {
    p.display().to_string().to_lowercase()
}

/// The photos of a folder to convert, with its subfolders (not too deep, not hidden ones); the files that already are
/// of the chosen format are left out (also the results of an earlier run).
fn walk(dir: &Path, depth: u32, format: Format, out: &mut Vec<PathBuf>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut list: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    list.sort();
    for p in list {
        // junctions and symbolic links are not followed: the same photos would come again under another path (on
        // Windows `is_symlink` is true for both – the name-surrogate reparse points – but not for OneDrive folders)
        let link = std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink());
        if p.is_dir() {
            if !link && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                walk(&p, depth + 1, format, out);
            }
        } else if convert::is_input(&p) && !format.is(&p) {
            out.push(p);
        }
    }
}
/// "1 photo" / "5 photos", "1 fotka" / "3 fotky" / "5 fotek".
fn count(n: usize) -> String {
    let word = if i18n::cz() { i18n::plural(n, "fotka", "fotky", "fotek") } else { i18n::plural(n, "photo", "photos", "photos") };
    format!("{n} {word}")
}

/// The same as the object of a verb ("Převést 1 fotku"); English does not change.
fn count_object(n: usize) -> String {
    let word = if i18n::cz() { i18n::plural(n, "fotku", "fotky", "fotek") } else { i18n::plural(n, "photo", "photos", "photos") };
    format!("{n} {word}")
}

/// A percentage: "42%" in English, "42 %" in Czech.
fn percent(v: &str) -> String {
    if i18n::cz() { format!("{v} %") } else { format!("{v}%") }
}

/// A file size as people read it: "820 kB", "3.4 MB" (Czech: "3,4 MB").
fn size_text(bytes: u64) -> String {
    let (v, unit) = if bytes >= 1_000_000_000 {
        (bytes as f64 / 1e9, "GB")
    } else if bytes >= 1_000_000 {
        (bytes as f64 / 1e6, "MB")
    } else {
        (bytes as f64 / 1e3, "kB")
    };
    let text = if unit == "kB" || v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    let text = if i18n::cz() { text.replace('.', ",") } else { text };
    format!("{text} {unit}")
}

/// File Explorer with the file selected.
fn show_in_explorer(file: &Path) -> std::io::Result<()> {
    std::process::Command::new(crate::deps::program("explorer.exe")).arg(format!("/select,{}", file.display())).spawn().map(drop)
}

/// The window as the owner of the file dialogs.
struct Parent(isize);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = std::num::NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(hwnd));
        // SAFETY: the handle is the main window of this app, which outlives the modal file dialog that borrows it.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for Parent {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: a Windows display handle holds no pointer; it is valid for as long as the process runs.
        Ok(unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Windows(WindowsDisplayHandle::new())) })
    }
}
