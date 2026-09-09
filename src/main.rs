#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use std::sync::Arc;

use app::ExcelLookupApp;
use eframe::egui;

/// 程序图标(窗口 / 任务栏)。读取 assets/icon.png,失败时退化为默认图标。
fn app_icon() -> Option<Arc<egui::IconData>> {
    let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icon.png"));
    match eframe::icon_data::from_png_bytes(bytes) {
        Ok(icon) => Some(Arc::new(icon)),
        Err(e) => {
            eprintln!("警告: 加载程序图标失败: {e}");
            None
        }
    }
}

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([1440.0, 900.0]);
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "ExcelLookup",
        options,
        Box::new(|cc| Ok(Box::new(ExcelLookupApp::new(cc)))),
    )
}
