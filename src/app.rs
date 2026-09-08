//! ExcelLookup 主应用界面 (egui)

use std::path::PathBuf;

use eframe::egui::{self, Color32};
use egui_extras::{Column, TableBuilder};

use excelookup_lib::join::{join, JoinSpec, JoinType, KeyMode};
use excelookup_lib::model::{CellValue, Table};

/// 一个已打开的数据源(文件 + sheets)
#[derive(Default, Clone)]
struct Source {
    path: Option<PathBuf>,
    /// 所有 sheet 名(工作簿顺序)
    sheet_names: Vec<String>,
    /// 所有 sheet 数据(与 sheet_names 对齐)
    sheets: Vec<Table>,
    /// 当前 sheet 下标
    sheet_idx: usize,
    error: Option<String>,
}

impl Source {
    fn is_loaded(&self) -> bool {
        !self.sheets.is_empty()
    }
    fn label(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
    /// 当前 sheet 名
    fn cur_sheet_name(&self) -> &str {
        self.sheet_names
            .get(self.sheet_idx)
            .map(|s| s.as_str())
            .unwrap_or("")
    }
    /// 当前 sheet 表
    fn cur_table(&self) -> Option<&Table> {
        self.sheets.get(self.sheet_idx)
    }
    fn cur_col_count(&self) -> usize {
        self.cur_table().map(|t| t.col_count()).unwrap_or(0)
    }
    fn cur_row_count(&self) -> usize {
        self.cur_table().map(|t| t.row_count()).unwrap_or(0)
    }
    fn cur_headers(&self) -> Vec<String> {
        self.cur_table().map(|t| t.headers.clone()).unwrap_or_default()
    }
}

pub struct ExcelLookupApp {
    left: Source,
    right: Source,
    join_type: JoinType,
    left_key_col: usize,
    right_key_col: usize,
    right_pick_cols: Vec<usize>,
    /// UI 用的宽松匹配开关(数字/文本互认 + trim)
    normalize_keys: bool,
    /// UI 用的括号归一化开关(中文/英文括号互认)
    bracket_fold: bool,
    result: Option<JoinOutcome>,
    /// 延迟到帧末处理(避免借用冲突)
    pending_open: Option<(Side, PathBuf)>,
    pending_save: bool,
}

struct JoinOutcome {
    table: Table,
    left_matched: usize,
    left_total: usize,
    right_matched_rows: usize,
    right_total: usize,
    out_rows: usize,
    err: Option<String>,
    join_type: JoinType,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

impl Default for ExcelLookupApp {
    fn default() -> Self {
        Self {
            left: Source::default(),
            right: Source::default(),
            join_type: JoinType::Left,
            left_key_col: 0,
            right_key_col: 0,
            right_pick_cols: vec![],
            normalize_keys: true,
            bracket_fold: true,
            result: None,
            pending_open: None,
            pending_save: false,
        }
    }
}

impl ExcelLookupApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_cjk_font(&cc.egui_ctx);
        Self::configure_ui_style(&cc.egui_ctx);
        Self::default()
    }

    fn key_mode(&self) -> KeyMode {
        KeyMode {
            number_text: self.normalize_keys,
            brackets: self.bracket_fold,
        }
    }

    fn pick_and_load(&mut self, side: Side) {
        let picked = rfd::FileDialog::new()
            .add_filter("Excel 工作簿", &["xlsx", "xls", "xlsb", "xlsm", "ods"])
            .add_filter("所有文件", &["*"])
            .pick_file();
        if let Some(p) = picked {
            self.pending_open = Some((side, p));
        }
    }

    fn open_file(&mut self, side: Side, path: PathBuf) {
        let src = match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        };
        src.error = None;
        match excelookup_lib::read_xlsx::read_workbook(&path) {
            Ok(sheets) => {
                // 至少保留一个(即使全空也存,便于用户切换看到空表)
                if sheets.is_empty() {
                    src.error = Some("工作簿中无工作表".into());
                    return;
                }
                let names: Vec<String> = sheets.iter().map(|(n, _)| n.clone()).collect();
                let tables: Vec<Table> = sheets.into_iter().map(|(_, t)| t).collect();
                src.path = Some(path);
                src.sheet_names = names;
                src.sheets = tables;
                src.sheet_idx = 0;
                // 跳到第一个非空 sheet
                if let Some(idx) = src.sheets.iter().position(|t| !t.is_empty()) {
                    src.sheet_idx = idx;
                }
            }
            Err(e) => {
                src.error = Some(format!("打开失败: {e}"));
            }
        }
        self.clamp_defaults();
        self.result = None;
    }

    /// 切换 sheet(切换后清结果、校正键列)
    fn switch_sheet(&mut self, side: Side, idx: usize) {
        let src = match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        };
        if idx >= src.sheets.len() {
            return;
        }
        src.sheet_idx = idx;
        self.clamp_defaults();
        self.result = None;
    }

    fn clamp_defaults(&mut self) {
        let lc = self.left.cur_col_count();
        let rc = self.right.cur_col_count();
        if lc > 0 && self.left_key_col >= lc {
            self.left_key_col = 0;
        }
        if rc > 0 {
            if self.right_key_col >= rc {
                self.right_key_col = 0;
            }
            self.right_pick_cols.retain(|&c| c < rc);
            if self.right_pick_cols.is_empty() && rc > 1 {
                self.right_pick_cols = vec![1];
            }
        }
    }

    fn run_join(&mut self) {
        if !self.left.is_loaded() || !self.right.is_loaded() {
            self.result = Some(JoinOutcome {
                table: Table::default(),
                left_matched: 0,
                left_total: 0,
                right_matched_rows: 0,
                right_total: 0,
                out_rows: 0,
                err: Some("请先加载两个数据源".into()),
                join_type: self.join_type,
            });
            return;
        }
        // 当前 sheet 的借用分离:先取引用
        let Some(left_t) = self.left.cur_table() else {
            return;
        };
        let Some(right_t) = self.right.cur_table() else {
            return;
        };
        let lc = left_t.col_count();
        let rc = right_t.col_count();
        if lc == 0 || rc == 0 {
            self.result = Some(JoinOutcome {
                table: Table::default(),
                left_matched: 0,
                left_total: 0,
                right_matched_rows: 0,
                right_total: 0,
                out_rows: 0,
                err: Some("当前工作表无列数据".into()),
                join_type: self.join_type,
            });
            return;
        }
        let lk = self.left_key_col.min(lc.saturating_sub(1));
        let rk = self.right_key_col.min(rc.saturating_sub(1));
        let rp: Vec<usize> = self
            .right_pick_cols
            .iter()
            .copied()
            .filter(|&c| c < rc)
            .collect();

        let spec = JoinSpec {
            join_type: self.join_type,
            left_keys: vec![lk],
            right_keys: vec![rk],
            right_pick: rp,
            key_mode: self.key_mode(),
        };
        let res = join(left_t, right_t, &spec);
        self.result = Some(JoinOutcome {
            table: res.table,
            left_matched: res.left_matched,
            left_total: res.left_total,
            right_matched_rows: res.right_matched_rows,
            right_total: res.right_total,
            out_rows: res.out_rows,
            err: None,
            join_type: self.join_type,
        });
    }

    fn export(&mut self) {
        let Some(res) = &self.result else { return };
        if res.err.is_some() || res.table.col_count() == 0 {
            return;
        }
        let table = res.table.clone();
        let picked = rfd::FileDialog::new()
            .add_filter("Excel 工作簿", &["xlsx"])
            .set_file_name("连接结果.xlsx")
            .save_file();
        if let Some(path) = picked {
            if let Err(e) = excelookup_lib::export::write_xlsx(&table, &path) {
                if let Some(r) = &mut self.result {
                    r.err = Some(format!("导出失败: {e}"));
                }
            }
        }
    }
}

impl eframe::App for ExcelLookupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            self.ui_header(ui);
            ui.add_space(8.0);
            self.ui_sources(ui);
            ui.add_space(8.0);
            self.ui_join_config(ui);
            ui.add_space(8.0);
            self.ui_stats(ui);
        });

        // 帧末处理延迟事件
        if let Some((side, path)) = self.pending_open.take() {
            self.open_file(side, path);
        }
        if self.pending_save {
            self.pending_save = false;
            self.export();
        }
    }
}

impl ExcelLookupApp {
    /// 保持字号易读，同时让设置区的间距和控件高度更紧凑。
    fn configure_ui_style(ctx: &egui::Context) {
        ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 5.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            style.spacing.interact_size = egui::vec2(32.0, 32.0);
            style.spacing.icon_width = 20.0;
            style.spacing.icon_width_inner = 14.0;
            style.spacing.icon_spacing = 5.0;
            style.spacing.combo_width = 160.0;
            style.spacing.window_margin = egui::Margin::same(10);

            style.text_styles.insert(
                egui::TextStyle::Small,
                egui::FontId::proportional(15.0),
            );
            style.text_styles.insert(
                egui::TextStyle::Body,
                egui::FontId::proportional(17.0),
            );
            style.text_styles.insert(
                egui::TextStyle::Button,
                egui::FontId::proportional(17.0),
            );
            style.text_styles.insert(
                egui::TextStyle::Monospace,
                egui::FontId::monospace(16.0),
            );
            style.text_styles.insert(
                egui::TextStyle::Heading,
                egui::FontId::proportional(24.0),
            );
        });
    }

    /// 统一的功能区容器，避免设置、结果等内容挤在一条横线上。
    fn section_frame<R>(
        ui: &mut egui::Ui,
        title: &str,
        hint: &str,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R> {
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(10))
            .fill(ui.visuals().faint_bg_color)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(title).strong().size(18.0));
                    if !hint.is_empty() {
                        ui.add_space(5.0);
                        ui.weak(hint);
                    }
                });
                ui.add_space(5.0);
                add_contents(ui)
            })
    }

    fn ui_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("ExcelLookup");
            ui.label(egui::RichText::new("Excel 双表连接").strong().size(18.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let has_res = self
                    .result
                    .as_ref()
                    .map(|r| r.err.is_none() && r.table.col_count() > 0)
                    .unwrap_or(false);
                if ui
                    .add_enabled(
                        has_res,
                        egui::Button::new("💾 导出结果…").min_size(egui::vec2(150.0, 36.0)),
                    )
                    .clicked()
                {
                    self.pending_save = true;
                }
            });
        });
    }

    fn ui_sources(&mut self, ui: &mut egui::Ui) {
        Self::section_frame(ui, "① 数据源", "分别加载主表和匹配表，可切换工作表", |ui| {
            ui.columns(2, |cols| {
                self.ui_source_card(&mut cols[0], Side::Left);
                self.ui_source_card(&mut cols[1], Side::Right);
            });
        });
    }

    fn ui_source_card(&mut self, ui: &mut egui::Ui, side: Side) {
        let title = match side {
            Side::Left => "数据源 A(主表)",
            Side::Right => "数据源 B(匹配表)",
        };
        // 先拷贝需要的数据,避免闭包内同时可变借用 self
        let (label, info, error): (String, Option<String>, Option<String>) = match side {
            Side::Left => (
                self.left_label(),
                self.left_info(),
                self.left.error.clone(),
            ),
            Side::Right => (
                self.right_label(),
                self.right_info(),
                self.right.error.clone(),
            ),
        };
        let (sheet_names, cur_idx, has_multi): (Vec<String>, usize, bool) = match side {
            Side::Left => (
                self.left.sheet_names.clone(),
                self.left.sheet_idx,
                self.left.sheets.len() > 1,
            ),
            Side::Right => (
                self.right.sheet_names.clone(),
                self.right.sheet_idx,
                self.right.sheets.len() > 1,
            ),
        };

        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(8))
            .fill(ui.visuals().window_fill())
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(title).strong().size(17.0));
                    if info.is_some() && error.is_none() {
                        ui.weak("已就绪");
                    }

                    ui.separator();
                    let file_button_width = ui.available_width().clamp(160.0, 520.0);
                    if ui
                        .add_sized([file_button_width, 32.0], egui::Button::new(label))
                        .clicked()
                    {
                        self.pick_and_load(side);
                    }

                    // sheet 下拉(多 sheet 时显示)
                    if has_multi && !sheet_names.is_empty() {
                        ui.separator();
                        ui.label("工作表");
                        let cur = sheet_names.get(cur_idx).cloned().unwrap_or_default();
                        egui::ComboBox::from_id_salt(match side {
                            Side::Left => "l_sheet",
                            Side::Right => "r_sheet",
                        })
                        .selected_text(cur)
                        .width(180.0)
                        .show_ui(ui, |ui| {
                            for (i, n) in sheet_names.iter().enumerate() {
                                if ui.selectable_label(i == cur_idx, n).clicked() {
                                    self.switch_sheet(side, i);
                                }
                            }
                        });
                    }
                });

                ui.add_space(3.0);
                if let Some(info) = &info {
                    ui.label(info);
                }
                if let Some(e) = &error {
                    ui.colored_label(Color32::LIGHT_RED, e);
                }
            });
    }

    fn left_label(&self) -> String {
        if self.left.is_loaded() {
            format!("📄 {}", self.left.label())
        } else {
            "选择 Excel 文件…".into()
        }
    }
    fn right_label(&self) -> String {
        if self.right.is_loaded() {
            format!("📄 {}", self.right.label())
        } else {
            "选择 Excel 文件…".into()
        }
    }
    fn left_info(&self) -> Option<String> {
        if !self.left.is_loaded() {
            return None;
        }
        Some(format!(
            "工作表 [{}]: {} 行 × {} 列 (共 {} 个工作表)",
            self.left.cur_sheet_name(),
            self.left.cur_row_count(),
            self.left.cur_col_count(),
            self.left.sheets.len()
        ))
    }
    fn right_info(&self) -> Option<String> {
        if !self.right.is_loaded() {
            return None;
        }
        Some(format!(
            "工作表 [{}]: {} 行 × {} 列 (共 {} 个工作表)",
            self.right.cur_sheet_name(),
            self.right.cur_row_count(),
            self.right.cur_col_count(),
            self.right.sheets.len()
        ))
    }

    fn ui_join_config(&mut self, ui: &mut egui::Ui) {
        Self::section_frame(ui, "② 连接设置", "定义连接方式、匹配键和结果列", |ui| {
            ui.columns(3, |cols| {
                cols[0].vertical(|ui| {
                    ui.label(egui::RichText::new("连接类型").strong());
                    ui.add_space(5.0);
                    egui::ComboBox::from_id_salt("join_type")
                        .selected_text(self.join_type.label())
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for jt in JoinType::all() {
                                ui.selectable_value(&mut self.join_type, jt, jt.label());
                            }
                        });
                });

                cols[1].vertical(|ui| {
                    ui.label(egui::RichText::new("A 键列（匹配列）").strong());
                    ui.add_space(5.0);
                    let headers = self.left.cur_headers();
                    Self::col_combo(ui, "a_key", &headers, &mut self.left_key_col);
                });

                cols[2].vertical(|ui| {
                    ui.label(egui::RichText::new("B 键列（匹配列）").strong());
                    ui.add_space(5.0);
                    let headers = self.right.cur_headers();
                    Self::col_combo(ui, "b_key", &headers, &mut self.right_key_col);
                });
            });

            ui.add_space(6.0);
            ui.columns(2, |cols| {
                cols[0].vertical(|ui| {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(7))
                        .fill(ui.visuals().window_fill())
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("匹配规则").strong().size(16.0));
                            ui.add_space(2.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.checkbox(
                                    &mut self.normalize_keys,
                                    "键宽松匹配（数字/文本互认，忽略首尾空格）",
                                );
                                ui.checkbox(
                                    &mut self.bracket_fold,
                                    "括号归一化（中文（）与英文()互认）",
                                );
                            });
                        });
                });

                cols[1].vertical(|ui| {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(7))
                        .fill(ui.visuals().window_fill())
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("输出列").strong().size(16.0));
                            ui.add_space(2.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.label("B 取值列");
                                let rc = self.right.cur_col_count();
                                if rc == 0 {
                                    ui.weak("加载 B 后可勾选");
                                } else {
                                    let headers = self.right.cur_headers();
                                    for i in 0..rc {
                                        if i == self.right_key_col {
                                            continue;
                                        }
                                        let mut checked = self.right_pick_cols.contains(&i);
                                        if ui.checkbox(&mut checked, &headers[i]).changed() {
                                            if checked {
                                                if !self.right_pick_cols.contains(&i) {
                                                    self.right_pick_cols.push(i);
                                                }
                                            } else {
                                                self.right_pick_cols.retain(|&c| c != i);
                                            }
                                        }
                                    }
                                    if self.right_pick_cols.is_empty() {
                                        ui.weak("未选取值列，结果将只有 A 的列");
                                    }
                                }
                            });
                        });
                });
            });

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_sized(
                        [140.0, 36.0],
                        egui::Button::new(egui::RichText::new("▶  执行连接").strong()),
                    )
                    .clicked()
                {
                    self.run_join();
                }
                if ui
                    .add_sized([120.0, 36.0], egui::Button::new("↺  清空结果"))
                    .clicked()
                {
                    self.result = None;
                }
                ui.weak("设置完成后执行连接");
            });
        });
    }

    /// 列选择下拉(带未加载禁用)
    fn col_combo(
        ui: &mut egui::Ui,
        id: &str,
        headers: &[String],
        sel: &mut usize,
    ) {
        if headers.is_empty() {
            ui.add_enabled(false, egui::Button::new("—"));
            return;
        }
        let sel_text = headers.get(*sel).cloned().unwrap_or_default();
        egui::ComboBox::from_id_salt(id)
            .selected_text(sel_text)
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (i, n) in headers.iter().enumerate() {
                    ui.selectable_value(sel, i, n);
                }
            });
    }

    fn ui_stats(&self, ui: &mut egui::Ui) {
        Self::section_frame(ui, "③ 连接结果", "查看命中统计、结果预览并导出文件", |ui| {
            let Some(res) = &self.result else {
                ui.weak("加载 A/B 两个数据源，选好键列后点击「执行连接」。");
                return;
            };
            if let Some(e) = &res.err {
                ui.colored_label(Color32::LIGHT_RED, format!("⚠ {e}"));
                return;
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(res.join_type.label()).strong());
                ui.separator();
                ui.label(format!(
                    "A：{} 行，匹配 {} 行",
                    res.left_total, res.left_matched
                ));
                ui.separator();
                ui.label(format!(
                    "B：{} 行，命中 {} 行",
                    res.right_total, res.right_matched_rows
                ));
                ui.separator();
                ui.label(format!("结果：{} 行", res.out_rows));
            });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label(egui::RichText::new("结果预览").strong().size(17.0));
            ui.add_space(6.0);
            self.ui_result_table(ui);
        });
    }

    fn ui_result_table(&self, ui: &mut egui::Ui) {
        let Some(res) = &self.result else { return };
        if res.err.is_some() || res.table.col_count() == 0 {
            return;
        }
        let table = &res.table;
        let headers = table.headers.clone();
        let ncols = table.col_count();
        let row_count = table.row_count();

        // 紧凑行高:正文行高 + 少量内边距,避免每行过高
        let text_h = ui.text_style_height(&egui::TextStyle::Body);
        let row_h = (text_h + 6.0).max(20.0);

        // 网格线颜色:用控件边框色(自动适配明/暗主题)
        let sep_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let sep_width = 1.0;

        // 表格自身带滚动,不再包外层 ScrollArea;
        // 列宽:除最后一列外按内容自适应,最后一列占满剩余宽度(解决表格不铺满窗格)
        let mut builder = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .vscroll(true)
            .max_scroll_height(f32::INFINITY);
        if ncols >= 2 {
            builder = builder
                .columns(Column::auto().at_least(60.0).clip(true), ncols - 1)
                .column(Column::remainder().at_least(80.0).clip(true));
        } else {
            builder = builder.columns(Column::auto().at_least(60.0).clip(true), ncols);
        }

        // 表头:底边加粗分隔线
        builder
            .header(row_h, |mut header| {
                for h in &headers {
                    header.col(|ui| {
                        ui.strong(h);
                    });
                }
            })
            .body(|body| {
                body.rows(row_h, row_count, |mut row| {
                    let ridx = row.index();
                    for c in 0..ncols {
                        row.col(|ui| {
                            let v = table
                                .cell(ridx, c)
                                .map(CellValue::display)
                                .unwrap_or_default();
                            ui.label(v);

                            // 在单元格自身 ui 上画网格线(借用安全):
                            // 底横线 + (非末列)右侧竖线
                            let rect = ui.max_rect();
                            let painter = ui.painter();
                            painter.hline(
                                rect.x_range(),
                                rect.bottom(),
                                egui::Stroke::new(sep_width, sep_color),
                            );
                            if c < ncols - 1 {
                                painter.vline(
                                    rect.right(),
                                    rect.y_range(),
                                    egui::Stroke::new(sep_width, sep_color),
                                );
                            }
                        });
                    }
                });
            });
    }
}

/// 安装 CJK 字体(运行时从系统加载,避免二进制膨胀)
fn install_cjk_font(ctx: &egui::Context) {
    use egui::FontDefinitions;

    let mut fonts = FontDefinitions::default();
    let candidates: &[&str] = if cfg!(windows) {
        &[
            "C:\\Windows\\Fonts\\msyh.ttc",
            "C:\\Windows\\Fonts\\msyhbd.ttc",
            "C:\\Windows\\Fonts\\simhei.ttf",
            "C:\\Windows\\Fonts\\simsun.ttc",
        ]
    } else {
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/wqy-microhei/wqy-microhei.ttc",
        ]
    };
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("cjk".to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push("cjk".to_owned());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}
