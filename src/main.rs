#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ExcelLookup",
        options,
        Box::new(|cc| Ok(Box::new(ExcelLookupApp::new(cc)))),
    )
}

// eframe 0.36: App trait 用 ui(&mut Ui, &mut Frame),不再有旧版 update(ctx)。

struct ExcelLookupApp;

impl ExcelLookupApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self
    }
}

impl eframe::App for ExcelLookupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("ExcelLookup — Excel 双表 Join");
            ui.label("骨架已就绪:下一步接入 Sheet 读取与表格预览。");
        });
    }
}
