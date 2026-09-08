#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use app::ExcelLookupApp;
use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([800.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ExcelLookup",
        options,
        Box::new(|cc| Ok(Box::new(ExcelLookupApp::new(cc)))),
    )
}
