//! The window of HeMiPix: the photos to convert on the left, the settings on the right, the progress, the File
//! Explorer switch and the Convert button at the bottom; the crop editor in the whole window. Laid out like
//! HeMiUnlocker and the settings of HeMiDock: a large title, small semibold section headers, cards with a frame.

use crate::convert::{self, CropRect, Done, Event, Fail, Format, Job, Preview};
use crate::crop::{Editor, Ratio};
use crate::i18n::{self, tr};
use crate::settings::{Existing, Output, Settings};
use crate::theme::{self, Palette, glyph};
use crate::widgets::{Btn, CARD_RADIUS, Tone, card, dropdown, glyph_box, info_bar, progress, row_layout, section, slider, toggle, two_lines};
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
/// The free extensions of the Microsoft Store that let Windows read HEIC.
const STORE_HEIF: &str = "ms-windows-store://pdp/?ProductId=9PMMSR1CGPWG";
const STORE_HEVC: &str = "ms-windows-store://pdp/?ProductId=9N4WGH0Z6VHQ";
/// The longer side of the preview in the crop editor.
const PREVIEW: u32 = 1800;

enum State {
    Waiting,
    Working,
    Done(Done),
    /// A JPG of the name exists and the setting says skip.
    Skipped,
    Failed(String),
}

struct Item {
    src: PathBuf,
    size: u64,
    state: State,
    crop: Option<CropRect>,
    /// The JPG made last time: converted again (after a new crop) it replaces it instead of a new name.
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
    About(bool),
    Open(String),
    OpenLicences,
    DismissToast,
    DismissCodec,
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
    menu_on: bool,
    about: bool,
    toast: Option<Toast>,
    /// Windows could not read a photo: the Store extensions are offered.
    no_codec: bool,
    /// Where the last converted photo went (for "Open folder").
    last_out: Option<PathBuf>,
    /// Mica of Windows 11 behind the window (the background is transparent then).
    mica: bool,
}

impl HeMiPixApp {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        let source = theme::read_source();
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
            menu_on: crate::shell_menu::is_on(),
            about: false,
            toast: None,
            no_codec: false,
            last_out: None,
            mica,
        };
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

    /// Where each waiting photo goes, by the settings; photos whose JPG exists are skipped when asked.
    fn plan(&mut self) -> Result<Vec<Job>, String> {
        let mut taken: HashSet<String> = HashSet::new();
        let mut jobs = Vec::new();
        for (id, item) in self.items.iter_mut().enumerate() {
            if !matches!(item.state, State::Waiting | State::Failed(_)) {
                continue;
            }
            // converted before (and cropped anew since): the same JPG again
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
        self.s.save();
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
                    let Some(item) = self.items.get_mut(i) else { continue };
                    item.state = match r {
                        Ok(d) => {
                            self.last_out = Some(d.out.clone());
                            item.last_out = Some(d.out.clone());
                            State::Done(d)
                        }
                        Err(Fail::NoCodec) => {
                            self.no_codec = true;
                            State::Failed(tr("Windows cannot read this photo.", "Windows tuhle fotku neumí přečíst.").into())
                        }
                        Err(Fail::Msg(m)) => State::Failed(m),
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
        self.toast = Some(Toast { text: text.into(), tone, until: Instant::now() + Duration::from_secs(6) });
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

    /// Light / dark or the accent colour changed (in Windows or in HeMiDock): the new colours at once.
    fn follow_theme(&mut self, ctx: &egui::Context) {
        if self.theme_checked.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.theme_checked = Instant::now();
        let source = theme::read_source();
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
                if Btn::new("").icon(glyph::BACK).subtle().tip(tr("Back", "Zpět")).show(ui, &p).clicked() {
                    acts.push(act);
                }
                ui.add_space(4.0);
            }
            let title = if self.about {
                tr("About", "O aplikaci")
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
                    && Btn::new("").icon(glyph::INFO).subtle().tip(tr("About", "O aplikaci")).show(ui, &p).clicked()
                {
                    acts.push(Action::About(true));
                }
            });
        });
    }

    fn about_page(&self, ui: &mut egui::Ui, acts: &mut Vec<Action>) {
        let p = self.p;
        section(ui, &p, "HeMiPix");
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
        value(ui, tr("Version", "Verze"), &format!("Beta {}", env!("CARGO_PKG_VERSION")));
        value(ui, tr("Build date", "Datum sestavení"), &i18n::build_date());
        let button = |ui: &mut egui::Ui, title: &str, detail: Option<&str>, label: &str, tip: &str, act: Action, acts: &mut Vec<Action>| {
            card(ui, &p, |ui| {
                row_layout(ui, 44.0, |ui| {
                    match detail {
                        Some(d) => two_lines(ui, &p, title, d),
                        None => {
                            ui.label(RichText::new(title).size(14.0).color(p.text));
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if Btn::new(label).tip(tip).show(ui, &p).clicked() {
                            acts.push(act);
                        }
                    });
                });
            });
        };
        button(ui, tr("Contact", "Kontakt"), None, tr("Report a problem", "Nahlásit problém"), CONTACT, Action::Open(format!("https://{CONTACT}")), acts);
        let whats_new = tr("The changes of every version and new downloads", "Změny každé verze a nové verze ke stažení");
        button(ui, tr("What's new", "Co je nového"), Some(whats_new), tr("Open", "Otevřít"), RELEASES, Action::Open(format!("https://{RELEASES}")), acts);
        let licences = tr("Licences of the components used", "Licence použitých součástí");
        button(ui, tr("Licences", "Licence"), Some(licences), tr("Open", "Otevřít"), "", Action::OpenLicences, acts);
        ui.add_space(16.0);
        let note = tr(
            "HEIC photos are read by Windows itself (HEIF Image Extensions and HEVC Video Extensions from the Microsoft Store). HeMiPix never goes online.",
            "Fotky HEIC čte samotný Windows (Rozšíření obrázků HEIF a Rozšíření videa HEVC z Microsoft Storu). HeMiPix se nikdy nepřipojuje k internetu.",
        );
        ui.add(egui::Label::new(RichText::new(note).size(12.0).color(p.text2)).wrap());
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
                    && Btn::new("").icon(glyph::OPEN_FOLDER).subtle().tip(tr("Show in File Explorer", "Ukázat v Průzkumníkovi")).show(ui, &p).clicked()
                {
                    acts.push(Action::ShowFile(d.out.clone()));
                }
                ui.add_space(6.0);
                // the result on the right
                match &item.state {
                    State::Done(d) => {
                        let less = 100.0 - d.out_size as f64 * 100.0 / d.in_size.max(1) as f64;
                        let pct = if less >= 0.5 { format!("−{less:.0} %") } else { format!("+{:.0} %", -less) };
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
                            "→ {} · {} × {}",
                            d.out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                            d.width,
                            d.height
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
            "HEIC, PNG, WebP … become JPG, PNG, WebP or TIFF – all at once, smaller and cropped if you wish.",
            "Z HEIC, PNG, WebP … udělá JPG, PNG, WebP nebo TIFF – všechny najednou, a když chceš, menší a oříznuté.",
        );
        painter.text(c + vec2(0.0, 34.0), Align2::CENTER_CENTER, hint, theme::body(13.0), p.text2);
    }

    /// The settings on the right: quality, where to save, what to do with an existing JPG, metadata.
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
                    two_lines(ui, &p, tr("Format", "Formát"), tr("What the photos become", "Na co se fotky převedou"));
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
                    ui.label(RichText::new(tr("Save to", "Uložit do")).size(14.0).color(p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let folder = self.s.format.label().to_uppercase();
                        let beside = if i18n::cz() { format!("Složky „{folder}“") } else { format!("Folder “{folder}”") };
                        let items = [tr("Next to the photos", "Vedle fotek"), beside.as_str(), tr("A chosen folder", "Vybrané složky")];
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
                    two_lines(ui, &p, tr("File exists", "Soubor už existuje"), tr("A file of the same name", "Soubor stejného jména"));
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
                    let detail = if self.s.format.has_exif() {
                        tr("Date, camera, location (Exif)", "Datum, fotoaparát, poloha (Exif)")
                    } else {
                        tr("Not carried into TIFF", "Do TIFF se nepřenášejí")
                    };
                    two_lines(ui, &p, tr("Metadata", "Metadata"), detail);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if toggle(ui, &p, self.s.keep_exif).clicked() {
                            self.s.keep_exif = !self.s.keep_exif;
                        }
                    });
                });
                if self.s.keep_exif {
                    row_layout(ui, 36.0, |ui| {
                        ui.label(RichText::new(tr("Without the location", "Bez polohy")).size(14.0).color(p.text));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if toggle(ui, &p, self.s.remove_gps).clicked() {
                                self.s.remove_gps = !self.s.remove_gps;
                            }
                        });
                    });
                }
            });
        });
        if self.s != before {
            self.s.save();
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
                if Btn::new(tr("HEIF Image Extensions", "Rozšíření obrázků HEIF")).icon(glyph::OPEN_FILE).show(ui, &p).clicked() {
                    acts.push(Action::Open(STORE_HEIF.into()));
                }
                if Btn::new(tr("HEVC Video Extensions", "Rozšíření videa HEVC")).icon(glyph::OPEN_FILE).show(ui, &p).clicked() {
                    acts.push(Action::Open(STORE_HEVC.into()));
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
                    let t = tr("{n} converted · {a} → {b}", "Převedeno {n} · {a} → {b}")
                        .replace("{n}", &count(done.len()))
                        .replace("{a}", &size_text(a))
                        .replace("{b}", &size_text(b));
                    ui.label(RichText::new(t).size(13.0).color(p.text));
                    if less >= 0.5 {
                        ui.label(RichText::new(format!("(−{less:.0} %)")).font(theme::semibold(13.0)).color(p.success));
                    }
                });
                ui.add_space(6.0);
            }
        }
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            let tip = tr(
                "Adds “Convert with HeMiPix” to the right-click menu of HEIC photos and folders. In Windows 11 it is under “Show more options”.",
                "Přidá „Převést v HeMiPixu“ do nabídky po kliknutí pravým tlačítkem na fotku HEIC nebo složku. Ve Windows 11 je pod „Zobrazit další možnosti“.",
            );
            if toggle(ui, &p, self.menu_on).on_hover_text(tip).clicked() {
                acts.push(Action::ToggleMenu);
            }
            ui.add_space(4.0);
            ui.label(RichText::new(tr("File Explorer context menu", "Kontextová nabídka Průzkumníka")).size(13.0).color(p.text)).on_hover_text(tip);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.run.is_some() {
                    if Btn::new(tr("Stop", "Zastavit")).icon(glyph::STOP).show(ui, &p).clicked() {
                        acts.push(Action::Stop);
                    }
                    return;
                }
                let pending = self.pending();
                let label = if pending > 0 { tr("Convert {n}", "Převést {n}").replace("{n}", &count(pending)) } else { tr("Convert", "Převést").to_string() };
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
                        self.s.save();
                    }
                }
                Action::OpenOutput => {
                    if let Some(f) = &self.last_out {
                        show_in_explorer(f);
                    }
                }
                Action::ShowFile(f) => show_in_explorer(&f),
                Action::ToggleMenu => match crate::shell_menu::set(!self.menu_on) {
                    Ok(()) => self.menu_on = crate::shell_menu::is_on(),
                    Err(e) => self.toast(e, Tone::Critical),
                },
                Action::About(on) => self.about = on,
                Action::Open(t) => shell_open(&t),
                Action::OpenLicences => {
                    let file = std::env::temp_dir().join("HeMiPix-ThirdPartyNotices.txt");
                    match std::fs::write(&file, include_str!("../assets/ThirdPartyNotices.txt")) {
                        Ok(()) => shell_open(&file.display().to_string()),
                        Err(e) => self.toast(e.to_string(), Tone::Critical),
                    }
                }
                Action::DismissToast => self.toast = None,
                Action::DismissCodec => self.no_codec = false,
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
                            // a converted photo cropped anew waits to be converted again (into the same JPG)
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
        if p.is_dir() {
            if !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                walk(&p, depth + 1, format, out);
            }
        } else if convert::is_input(&p) && !format.is(&p) {
            out.push(p);
        }
    }
}
/// "1 photo" / "5 photos", "1 fotka" / "3 fotky" / "5 fotek".
fn count(n: usize) -> String {
    if i18n::cz() {
        let word = match n {
            1 => "fotka",
            2..=4 => "fotky",
            _ => "fotek",
        };
        format!("{n} {word}")
    } else {
        format!("{n} {}", if n == 1 { "photo" } else { "photos" })
    }
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

fn shell_open(target: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, t) = (w("open"), w(target));
    unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), t.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
}

/// File Explorer with the file selected.
fn show_in_explorer(file: &Path) {
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", file.display())).spawn();
}

/// The window as the owner of the file dialogs.
struct Parent(isize);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = std::num::NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(hwnd));
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for Parent {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Windows(WindowsDisplayHandle::new())) })
    }
}
