// HeMiPix – converts HEIC photos (from iPhones) to JPG in batches; smaller and cropped if wanted.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod container;
mod convert;
mod crop;
mod deps;
mod i18n;
mod launcher;
mod inbox;
mod info;
mod settings;
mod shell_menu;
mod theme;
mod widgets;

use eframe::egui;
use std::path::PathBuf;

fn main() -> eframe::Result {
    // photos / folders given on the command line (also the way the File Explorer item starts it)
    let paths: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .map(|p| std::path::absolute(&p).unwrap_or(p))
        .collect();

    // HeMiPix runs already: it gets the photos, this one ends
    if !inbox::first() {
        inbox::send(&paths);
        return Ok(());
    }
    // what the first window left unread last time is old
    let _ = inbox::take();
    shell_menu::repair();

    let icon = egui::IconData { rgba: include_bytes!("../assets/HeMiPix-64.rgba").to_vec(), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("HeMiPix")
            .with_app_id("HeMiPix")
            .with_inner_size([980.0, 660.0])
            .with_min_inner_size([760.0, 520.0])
            .with_icon(icon)
            .with_drag_and_drop(true)
            // transparent, so the Mica of Windows 11 shows around the cards
            .with_transparent(true),
        centered: true,
        ..Default::default()
    };
    eframe::run_native("HeMiPix", options, Box::new(move |cc| Ok(Box::new(app::HeMiPixApp::new(cc, paths)))))
}
