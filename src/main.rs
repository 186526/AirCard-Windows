#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod airlift;
mod airtraffic;
mod app;
mod backend;
mod device;
mod flasher;
mod grappa;
mod image_skin;
mod passthm;
mod scanner;

/// Window and taskbar title. The version comes from `Cargo.toml`, which is the
/// only place it is written down.
pub const APP_TITLE: &str = concat!("AirCard v", env!("CARGO_PKG_VERSION"));

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 620.0])
            .with_min_inner_size([850.0, 560.0])
            .with_title(APP_TITLE),
        ..Default::default()
    };

    eframe::run_native(
        APP_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(app::AirCardApp::new(cc)))),
    )
}
