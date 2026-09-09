//! ExcelLookup 主应用界面 (egui)

use std::path::PathBuf;

use eframe::egui::{self, Color32, CornerRadius, Shadow, Stroke};
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

    fn cur_sheet_name(&self) -> &str {
        self.sheet_names
            .get(self.sheet_idx)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

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
        self.cur_table()
            .map(|t| t.headers.clone())
            .unwrap_or_default()
    }
}

pub struct ExcelLookupApp {
    left: Source,
    right: Source,
    /// 当前工作流步骤:主工作区一次只展示一个步骤。
    step: WorkflowStep,
    join_type: JoinType,
    /// None = 未选择(该侧表被替换/切换后需重新选择)
    left_key_col: Option<usize>,
    right_key_col: Option<usize>,
    right_pick_cols: Vec<usize>,
    /// UI 用的宽松匹配开关(数字/文本互认 + trim)
    normalize_keys: bool,
    /// UI 用的括号归一化开关(中文/英文括号互认)
    bracket_fold: bool,
    result: Option<JoinOutcome>,
    /// 结果预览中的文本筛选条件(仅影响 UI,不改变导出内容)。
    result_filter: String,
    /// 延迟到帧末处理(避免借用冲突)
    pending_open: Option<(Side, PathBuf)>,
    pending_save: bool,
    /// 对调 A/B 后帧末统一处理(重置键列/输出列/结果)
    pending_swap: bool,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkflowStep {
    Sources,
    Configure,
    Result,
}

impl WorkflowStep {
    fn number(self) -> usize {
        match self {
            Self::Sources => 1,
            Self::Configure => 2,
            Self::Result => 3,
        }
    }

    fn eyebrow(self) -> &'static str {
        match self {
            Self::Sources => "第一步 · 数据源",
            Self::Configure => "第二步 · 连接配置",
            Self::Result => "第三步 · 结果预览",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Sources => "先把要连接的两张表放在一起",
            Self::Configure => "选择两张表如何对齐",
            Self::Result => "检查结果，确认后导出",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Sources => "文件只在本机读取，确认工作表后进入连接配置。",
            Self::Configure => "选中匹配列，再确定需要带出的字段。",
            Self::Result => "先看命中情况，再保存为新的工作簿。",
        }
    }
}

impl Default for ExcelLookupApp {
    fn default() -> Self {
        Self {
            left: Source::default(),
            right: Source::default(),
            step: WorkflowStep::Sources,
            join_type: JoinType::Left,
            left_key_col: None,
            right_key_col: None,
            right_pick_cols: vec![],
            normalize_keys: true,
            bracket_fold: true,
            result: None,
            result_filter: String::new(),
            pending_open: None,
            pending_save: false,
            pending_swap: false,
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
            self.step = WorkflowStep::Sources;
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
        // 该侧表已整体替换:键列/输出列需重新选择
        self.reset_side_on_source_change(side);
        self.result = None;
        self.result_filter.clear();
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
        // 该侧表已切换:键列/输出列需重新选择
        self.reset_side_on_source_change(side);
        self.result = None;
        self.result_filter.clear();
        if self.step == WorkflowStep::Result {
            self.step = WorkflowStep::Configure;
        }
    }

    /// 某侧的表被替换/切换后调用:清空该侧键列选择(严格版:即使下标合法也不保留,避免
    /// "下标合法但列含义已变"的静默错误),并清空该侧输出列(若为 B)。另一侧不受影响。
    fn reset_side_on_source_change(&mut self, side: Side) {
        match side {
            Side::Left => {
                self.left_key_col = None;
            }
            Side::Right => {
                self.right_key_col = None;
                self.right_pick_cols.clear();
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
            self.step = WorkflowStep::Configure;
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
            self.step = WorkflowStep::Configure;
            return;
        }
        // 键列未选择或当前表无列时不允许执行
        let (Some(lk), Some(rk)) = (self.left_key_col, self.right_key_col) else {
            self.result = Some(JoinOutcome {
                table: Table::default(),
                left_matched: 0,
                left_total: 0,
                right_matched_rows: 0,
                right_total: 0,
                out_rows: 0,
                err: Some("请先在连接配置中选择 A/B 匹配列".into()),
                join_type: self.join_type,
            });
            self.step = WorkflowStep::Configure;
            return;
        };
        let lk = lk.min(lc.saturating_sub(1));
        let rk = rk.min(rc.saturating_sub(1));
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
        self.result_filter.clear();
        self.step = WorkflowStep::Result;
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

    fn clear_result(&mut self) {
        self.result = None;
        self.result_filter.clear();
        if self.sources_ready() {
            self.step = WorkflowStep::Configure;
        }
    }

    fn clear_sources(&mut self) {
        self.left = Source::default();
        self.right = Source::default();
        self.left_key_col = None;
        self.right_key_col = None;
        self.right_pick_cols.clear();
        self.result = None;
        self.result_filter.clear();
        self.step = WorkflowStep::Sources;
    }

    /// 对调 A/B 两个数据源(文件+sheet+当前选中)。
    /// 匹配列随对调交换(新 A 沿用原 B 的键列,新 B 沿用原 A 的):整表互换后列号有效性自动
    /// 守恒,即便某侧此前未选择,交换后仍为 None。
    /// 输出列清空——主从关系已变,带出字段需重新确认。
    fn swap_sources(&mut self) {
        std::mem::swap(&mut self.left, &mut self.right);
        std::mem::swap(&mut self.left_key_col, &mut self.right_key_col);
        self.right_pick_cols.clear();
        self.result = None;
        self.result_filter.clear();
        self.step = WorkflowStep::Sources;
    }

    fn sources_ready(&self) -> bool {
        self.left.is_loaded()
            && self.right.is_loaded()
            && self.left.error.is_none()
            && self.right.error.is_none()
    }

    fn result_ready(&self) -> bool {
        self.result
            .as_ref()
            .map(|r| r.err.is_none() && r.table.col_count() > 0)
            .unwrap_or(false)
    }

    fn can_enter_step(&self, step: WorkflowStep) -> bool {
        match step {
            WorkflowStep::Sources => true,
            WorkflowStep::Configure => self.sources_ready(),
            WorkflowStep::Result => self.result_ready(),
        }
    }

    fn go_to_step(&mut self, step: WorkflowStep) {
        if self.can_enter_step(step) {
            self.step = step;
        }
    }
}

impl eframe::App for ExcelLookupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::left("workflow_sidebar")
            .exact_size(224.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Self::navy())
                    .inner_margin(egui::Margin::symmetric(16, 25)),
            )
            .show(ui, |ui| self.ui_sidebar(ui));

        egui::Panel::top("topbar")
            .exact_size(66.0)
            .frame(
                egui::Frame::new()
                    .fill(Self::white())
                    .stroke(Stroke::new(1.0, Self::line()))
                    .inner_margin(egui::Margin::symmetric(35, 0)),
            )
            .show(ui, |ui| self.ui_topbar(ui));

        egui::CentralPanel::default()
            // 在导航栏与主工作区之间保留独立的浅色留白，避免内容贴边。
            .frame(
                egui::Frame::new()
                    .fill(Self::canvas())
                    .inner_margin(egui::Margin::symmetric(16, 0)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("workspace_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        self.ui_workspace(ui);
                    });
            });

        // 帧末处理延迟事件
        if let Some((side, path)) = self.pending_open.take() {
            self.open_file(side, path);
        }
        if self.pending_save {
            self.pending_save = false;
            self.export();
        }
        if self.pending_swap {
            self.pending_swap = false;
            self.swap_sources();
        }
    }
}

impl ExcelLookupApp {
    // ---------- 颜色与基础组件 ----------

    fn navy() -> Color32 {
        Color32::from_rgb(23, 39, 59)
    }

    fn navy_2() -> Color32 {
        Color32::from_rgb(32, 52, 77)
    }

    fn sidebar_text() -> Color32 {
        Color32::from_rgb(231, 238, 247)
    }

    fn sidebar_muted() -> Color32 {
        Color32::from_rgb(144, 165, 187)
    }

    fn sidebar_faint() -> Color32 {
        Color32::from_rgb(129, 148, 170)
    }

    fn ink() -> Color32 {
        Color32::from_rgb(31, 48, 66)
    }

    fn muted() -> Color32 {
        Color32::from_rgb(113, 129, 150)
    }

    fn soft() -> Color32 {
        Color32::from_rgb(149, 165, 181)
    }

    fn canvas() -> Color32 {
        Color32::from_rgb(237, 242, 247)
    }

    fn white() -> Color32 {
        Color32::WHITE
    }

    fn surface() -> Color32 {
        Color32::from_rgb(251, 252, 254)
    }

    fn line() -> Color32 {
        Color32::from_rgb(220, 229, 238)
    }

    fn line_strong() -> Color32 {
        Color32::from_rgb(201, 214, 227)
    }

    fn blue() -> Color32 {
        Color32::from_rgb(43, 104, 197)
    }

    fn blue_soft() -> Color32 {
        Color32::from_rgb(234, 242, 255)
    }

    fn teal() -> Color32 {
        Color32::from_rgb(35, 139, 120)
    }

    fn teal_soft() -> Color32 {
        Color32::from_rgb(228, 246, 241)
    }

    fn amber() -> Color32 {
        Color32::from_rgb(189, 116, 47)
    }

    fn card_frame(fill: Color32, stroke: Color32, margin: i8) -> egui::Frame {
        egui::Frame::new()
            .inner_margin(egui::Margin::same(margin))
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CornerRadius::ZERO)
    }

    fn panel_frame() -> egui::Frame {
        egui::Frame::new()
            .inner_margin(egui::Margin::ZERO)
            .fill(Self::white())
            .stroke(Stroke::new(1.0, Self::line()))
            .corner_radius(CornerRadius::ZERO)
            .shadow(Shadow {
                offset: [0, 3],
                blur: 12,
                spread: 0,
                color: Color32::from_black_alpha(16),
            })
    }

    fn status_badge(ui: &mut egui::Ui, text: &str) {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(8, 4))
            .fill(Self::teal_soft())
            .corner_radius(CornerRadius::ZERO)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 3.0, Self::teal());
                    ui.label(egui::RichText::new(text).size(13.0).color(Self::teal()));
                });
            });
    }

    fn letter_badge(ui: &mut egui::Ui, letter: &str, color: Color32) {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(9, 6))
            .fill(color)
            .corner_radius(CornerRadius::ZERO)
            .show(ui, |ui| {
                ui.label(egui::RichText::new(letter).strong().size(15.0).color(Color32::WHITE));
            });
    }

    fn primary_button(ui: &mut egui::Ui, text: &str, width: f32, enabled: bool) -> egui::Response {
        ui.add_enabled(
            enabled,
            egui::Button::new(egui::RichText::new(text).strong().color(Color32::WHITE))
                .min_size(egui::vec2(width, 36.0))
                .fill(Self::blue())
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::ZERO),
        )
    }

    fn green_button(ui: &mut egui::Ui, text: &str, width: f32) -> egui::Response {
        ui.add(
            egui::Button::new(egui::RichText::new(text).strong().color(Color32::WHITE))
                .min_size(egui::vec2(width, 42.0))
                .fill(Self::teal())
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::ZERO),
        )
    }

    fn secondary_button(ui: &mut egui::Ui, text: &str, width: f32) -> egui::Response {
        ui.add(
            egui::Button::new(egui::RichText::new(text).color(Self::muted()))
                .min_size(egui::vec2(width, 36.0))
                .fill(Self::white())
                .stroke(Stroke::new(1.0, Self::line_strong()))
                .corner_radius(CornerRadius::ZERO),
        )
    }

    fn action_row(ui: &mut egui::Ui, note: &str, add_buttons: impl FnOnce(&mut egui::Ui)) {
        ui.add_space(17.0);
        ui.separator();
        ui.add_space(15.0);
        ui.horizontal(|ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().circle_stroke(
                    rect.center(),
                    5.0,
                    Stroke::new(1.4, Self::teal()),
                );
                ui.painter().line_segment(
                    [
                        egui::pos2(rect.center().x, rect.center().y),
                        egui::pos2(rect.center().x + 2.5, rect.center().y + 2.0),
                    ],
                    Stroke::new(1.2, Self::teal()),
                );
                ui.label(egui::RichText::new(note).size(13.0).color(Self::muted()));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_buttons);
        });
    }

    fn work_panel(
        ui: &mut egui::Ui,
        index: &str,
        title: &str,
        hint: &str,
        status: Option<&str>,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) {
        Self::panel_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(index)
                        .size(13.0)
                        .strong()
                        .color(Self::blue()),
                );
                ui.label(egui::RichText::new(title).size(17.0).strong().color(Self::ink()));
                ui.label(egui::RichText::new(hint).size(13.0).color(Self::muted()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(status) = status {
                        Self::status_badge(ui, status);
                    }
                });
            });
            ui.separator();
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(18, 18))
                .show(ui, add_contents);
        });
    }

    fn sub_panel(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
        Self::card_frame(Self::surface(), Self::line(), 16).show(ui, add_contents);
    }

    // ---------- 外壳与工作流 ----------

    fn ui_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            Self::brand_mark(ui);
            ui.add_space(1.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("ExcelLookup")
                        .size(19.0)
                        .strong()
                        .color(Self::sidebar_text()),
                );
                ui.label(
                    egui::RichText::new("双表连接工具")
                        .size(13.0)
                        .color(Self::sidebar_muted()),
                );
            });
        });

        ui.add_space(25.0);
        let (rule_rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 1.0),
            egui::Sense::hover(),
        );
        ui.painter().hline(
            rule_rect.x_range(),
            rule_rect.center().y,
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(228, 239, 250, 28)),
        );
        ui.add_space(24.0);
        ui.label(
            egui::RichText::new("工作流程")
                .size(13.0)
                .strong()
                .color(Self::sidebar_faint()),
        );
        ui.add_space(13.0);

        self.ui_workflow_step(ui, WorkflowStep::Sources);
        self.ui_workflow_step(ui, WorkflowStep::Configure);
        self.ui_workflow_step(ui, WorkflowStep::Result);

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "ExcelLookup {}",
                    option_env!("EXCELOOKUP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
                ))
                    .size(12.0)
                    .color(Self::sidebar_faint()),
            );
        });
    }

    fn brand_mark(ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, Self::blue());
        let left = rect.left() + 10.0;
        let right = rect.right() - 10.0;
        let top = rect.top() + 10.0;
        let bottom = rect.bottom() - 10.0;
        let mid = rect.center().y;
        let mark = Stroke::new(1.6, Color32::WHITE);
        painter.line_segment(
            [egui::pos2(left, top), egui::pos2(right, top)],
            mark,
        );
        painter.line_segment(
            [egui::pos2(left, bottom), egui::pos2(right, bottom)],
            mark,
        );
        painter.line_segment(
            [egui::pos2(left, top), egui::pos2(rect.center().x, mid)],
            mark,
        );
        painter.line_segment(
            [egui::pos2(right, top), egui::pos2(rect.center().x, mid)],
            mark,
        );
        painter.line_segment(
            [egui::pos2(rect.center().x, mid), egui::pos2(left, bottom)],
            mark,
        );
        painter.line_segment(
            [egui::pos2(rect.center().x, mid), egui::pos2(right, bottom)],
            mark,
        );
    }

    fn ui_workflow_step(&mut self, ui: &mut egui::Ui, step: WorkflowStep) {
        let active = self.step == step;
        let done = step.number() < self.step.number();
        let enabled = self.can_enter_step(step);
        let (title, hint) = match step {
            WorkflowStep::Sources => {
                if self.sources_ready() {
                    ("数据源", "已加载 2 个文件")
                } else {
                    ("数据源", "加载并确认两张表")
                }
            }
            WorkflowStep::Configure => {
                if self.sources_ready() {
                    ("连接配置", "选择匹配键与输出列")
                } else {
                    ("连接配置", "等待数据源")
                }
            }
            WorkflowStep::Result => {
                if self.result_ready() {
                    ("结果预览", "检查命中情况并导出")
                } else {
                    ("结果预览", "执行连接后查看")
                }
            }
        };

        let frame = if active {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(8, 10))
                .fill(Self::navy_2())
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(116, 170, 226, 82),
                ))
                .corner_radius(CornerRadius::ZERO)
        } else {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 10))
        };
        let inner = frame.show(ui, |ui| {
            ui.set_min_height(39.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                if done {
                    ui.painter().circle_filled(rect.center(), 7.5, Self::teal());
                    ui.painter().line_segment(
                        [
                            egui::pos2(rect.center().x - 3.0, rect.center().y),
                            egui::pos2(rect.center().x - 0.5, rect.center().y + 2.5),
                        ],
                        Stroke::new(1.4, Self::navy()),
                    );
                    ui.painter().line_segment(
                        [
                            egui::pos2(rect.center().x - 0.5, rect.center().y + 2.5),
                            egui::pos2(rect.center().x + 4.0, rect.center().y - 3.0),
                        ],
                        Stroke::new(1.4, Self::navy()),
                    );
                } else if active {
                    ui.painter().circle_filled(rect.center(), 7.5, Self::navy());
                    ui.painter().circle_stroke(
                        rect.center(),
                        7.5,
                        Stroke::new(3.5, Color32::from_rgb(121, 169, 229)),
                    );
                } else {
                    ui.painter().circle_stroke(
                        rect.center(),
                        7.5,
                        Stroke::new(1.0, Color32::from_rgb(109, 145, 179)),
                    );
                }
                ui.vertical(|ui| {
                    let title_color = if enabled {
                        Self::sidebar_text()
                    } else {
                        Self::sidebar_faint()
                    };
                    let hint_color = if active {
                        Color32::from_rgb(184, 204, 227)
                    } else {
                        Self::sidebar_muted()
                    };
                    ui.label(egui::RichText::new(title).size(15.0).strong().color(title_color));
                    ui.add_space(3.0);
                    ui.label(egui::RichText::new(hint).size(13.0).color(hint_color));
                });
            });
        });
        let response = ui.interact(
            inner.response.rect,
            ui.id().with(("workflow-step", step.number())),
            egui::Sense::click(),
        );
        if enabled && response.clicked() {
            self.go_to_step(step);
        }
    }

    fn ui_topbar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("连接工作台").strong().color(Self::ink()));
            ui.label(egui::RichText::new("/").color(Self::soft()));
            ui.label(egui::RichText::new("新建连接").color(Self::muted()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, Self::teal());
                ui.label(egui::RichText::new("本地运行中").size(14.0).color(Self::teal()));
                ui.add_space(17.0);
                let response = ui.add(
                    egui::Button::new(egui::RichText::new("?").size(14.0).color(Self::muted()))
                        .min_size(egui::vec2(26.0, 26.0))
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::ZERO),
                );
                response.on_hover_text("查看使用说明");
            });
        });
    }

    fn ui_workspace(&mut self, ui: &mut egui::Ui) {
        self.ui_page_heading(ui);
        ui.add_space(18.0);

        if self.step != WorkflowStep::Sources && self.sources_ready() {
            self.ui_context_strip(ui);
            ui.add_space(13.0);
        }

        match self.step {
            WorkflowStep::Sources => self.ui_step_sources(ui),
            WorkflowStep::Configure => self.ui_step_config(ui),
            WorkflowStep::Result => self.ui_step_result(ui),
        }
    }

    fn ui_page_heading(&mut self, ui: &mut egui::Ui) {
        let show_export = self.step == WorkflowStep::Result && self.result_ready();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(self.step.eyebrow())
                        .size(13.0)
                        .strong()
                        .color(Self::blue()),
                );
                ui.add_space(5.0);
                ui.label(
                    egui::RichText::new(self.step.title())
                        .size(30.0)
                        .strong()
                        .color(Self::ink()),
                );
                ui.add_space(5.0);
                ui.label(
                    egui::RichText::new(self.step.description())
                        .size(15.0)
                        .color(Self::muted()),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
                if show_export {
                    if Self::green_button(ui, "导出结果  ↓", 136.0).clicked() {
                        self.pending_save = true;
                    }
                    ui.add_space(16.0);
                }
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("♢").size(18.0).color(Self::blue()));
                    ui.label(
                        egui::RichText::new("适用于 VLOOKUP 与多表合并")
                            .size(13.0)
                            .color(Self::muted()),
                    );
                });
            });
        });
    }

    fn ui_context_strip(&mut self, ui: &mut egui::Ui) {
        let left = self.source_context_data(Side::Left);
        let right = self.source_context_data(Side::Right);
        Self::card_frame(Self::white(), Self::line(), 13).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                Self::context_source(ui, Side::Left, &left.0, &left.1, left.2, left.3);
                ui.separator();
                Self::context_source(ui, Side::Right, &right.0, &right.1, right.2, right.3);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if Self::secondary_button(ui, "更换数据源  ↻", 112.0).clicked() {
                        self.go_to_step(WorkflowStep::Sources);
                    }
                });
            });
        });
    }

    fn source_context_data(&self, side: Side) -> (String, String, usize, usize) {
        let src = match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        };
        (
            format!("{} · {}", src.label(), src.cur_sheet_name()),
            format!("{} 行 · {} 列", src.cur_row_count(), src.cur_col_count()),
            src.cur_row_count(),
            src.cur_col_count(),
        )
    }

    fn context_source(
        ui: &mut egui::Ui,
        side: Side,
        label: &str,
        info: &str,
        _rows: usize,
        _cols: usize,
    ) {
        ui.horizontal(|ui| {
            Self::letter_badge(
                ui,
                match side {
                    Side::Left => "A",
                    Side::Right => "B",
                },
                match side {
                    Side::Left => Self::blue(),
                    Side::Right => Self::teal(),
                },
            );
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(label).size(13.0).strong().color(Self::ink()));
                ui.label(egui::RichText::new(info).size(12.0).color(Self::muted()));
            });
        });
    }

    // ---------- 第一步:数据源 ----------

    fn ui_step_sources(&mut self, ui: &mut egui::Ui) {
        let status = if self.sources_ready() {
            Some("已加载 2 个文件")
        } else {
            None
        };
        Self::work_panel(
            ui,
            "01",
            "加载数据源",
            "先确认需要连接的两张表",
            status,
            |ui| self.ui_sources_body(ui),
        );
    }

    fn ui_sources_body(&mut self, ui: &mut egui::Ui) {
        ui.columns(3, |cols| {
            self.ui_source_card(&mut cols[0], Side::Left);
            // 中间列:对调 A/B 按钮(需要整列高度与两侧卡片对齐)
            cols[1].vertical_centered(|ui| {
                ui.add_space(8.0);
                let size = egui::vec2(48.0, 48.0);
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                let button_rect = egui::Rect::from_center_size(rect.center(), size);
                // 圆底 + 双箭头(⇄) + 悬停加深
                let hovered = ui.rect_contains_pointer(button_rect);
                let (bg, fg) = if hovered {
                    (Self::blue_soft(), Self::blue())
                } else {
                    (Self::white(), Self::muted())
                };
                let painter = ui.painter();
                painter.circle_filled(button_rect.center(), 24.0, bg);
                painter.circle_stroke(button_rect.center(), 24.0, Stroke::new(1.0, Self::line_strong()));
                let center = button_rect.center();
                // 上箭头(右向)
                let y1 = center.y - 9.0;
                painter.line_segment(
                    [egui::pos2(center.x - 8.0, y1), egui::pos2(center.x + 8.0, y1)],
                    Stroke::new(2.0, fg),
                );
                painter.line_segment(
                    [egui::pos2(center.x + 8.0, y1), egui::pos2(center.x + 3.0, y1 - 4.0)],
                    Stroke::new(2.0, fg),
                );
                painter.line_segment(
                    [egui::pos2(center.x + 8.0, y1), egui::pos2(center.x + 3.0, y1 + 4.0)],
                    Stroke::new(2.0, fg),
                );
                // 下箭头(左向)
                let y2 = center.y + 9.0;
                painter.line_segment(
                    [egui::pos2(center.x - 8.0, y2), egui::pos2(center.x + 8.0, y2)],
                    Stroke::new(2.0, fg),
                );
                painter.line_segment(
                    [egui::pos2(center.x - 8.0, y2), egui::pos2(center.x - 3.0, y2 - 4.0)],
                    Stroke::new(2.0, fg),
                );
                painter.line_segment(
                    [egui::pos2(center.x - 8.0, y2), egui::pos2(center.x - 3.0, y2 + 4.0)],
                    Stroke::new(2.0, fg),
                );

                let resp = ui.interact(button_rect, ui.id().with("swap_ab"), egui::Sense::click());
                if resp.clicked() {
                    self.pending_swap = true;
                }
                resp.on_hover_text("对调 A/B 表");
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("对调")
                        .size(12.0)
                        .color(if hovered { Self::blue() } else { Self::soft() }),
                );
            });
            self.ui_source_card(&mut cols[2], Side::Right);
        });

        let can_next = self.sources_ready();
        Self::action_row(ui, "文件只在本机读取，不会上传", |ui| {
            if Self::primary_button(ui, "下一步：配置连接  →", 152.0, can_next).clicked() {
                self.go_to_step(WorkflowStep::Configure);
            }
            if Self::secondary_button(ui, "清空数据源", 104.0).clicked() {
                self.clear_sources();
            }
        });
    }

    fn ui_source_card(&mut self, ui: &mut egui::Ui, side: Side) {
        let (loaded, label, rows, cols, sheet_names, sheet_idx, error) = {
            let src = match side {
                Side::Left => &self.left,
                Side::Right => &self.right,
            };
            (
                src.is_loaded(),
                src.label(),
                src.cur_row_count(),
                src.cur_col_count(),
                src.sheet_names.clone(),
                src.sheet_idx,
                src.error.clone(),
            )
        };
        let accent = match side {
            Side::Left => Self::blue(),
            Side::Right => Self::teal(),
        };
        let role = match side {
            Side::Left => ("主表", "需要保留的完整数据"),
            Side::Right => ("匹配表", "提供需要带出的字段"),
        };

        Self::card_frame(Self::surface(), Self::line(), 16).show(ui, |ui| {
            ui.set_min_height(238.0);
            ui.horizontal(|ui| {
                Self::letter_badge(
                    ui,
                    match side {
                        Side::Left => "A",
                        Side::Right => "B",
                    },
                    accent,
                );
                ui.add_space(1.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(role.0).size(16.0).strong().color(Self::ink()));
                        ui.label(egui::RichText::new(role.1).size(13.0).color(Self::muted()));
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if loaded && error.is_none() {
                        Self::status_badge(ui, "已加载");
                    }
                });
            });
            ui.add_space(19.0);

            let file_label = if loaded {
                format!("▣  {label}")
            } else {
                "选择工作簿".to_owned()
            };
            let button_width = ui.available_width();
            if ui
                .add_sized(
                    [button_width, 39.0],
                    egui::Button::new(egui::RichText::new(file_label).strong().color(Self::ink()))
                        .fill(Self::white())
                        .stroke(Stroke::new(1.0, Self::line_strong()))
                .corner_radius(CornerRadius::ZERO),
                )
                .clicked()
            {
                self.pick_and_load(side);
            }

            if loaded {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("工作簿 · {rows} 行 · {cols} 列"))
                        .size(13.0)
                        .color(Self::muted()),
                );
                ui.add_space(17.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("工作表").size(13.0).color(Self::soft()));
                    ui.add_space(8.0);
                    let current = sheet_names
                        .get(sheet_idx)
                        .cloned()
                        .unwrap_or_else(|| "未选择".into());
                    egui::ComboBox::from_id_salt(match side {
                        Side::Left => "workflow_left_sheet",
                        Side::Right => "workflow_right_sheet",
                    })
                    .selected_text(current)
                    .width((ui.available_width() - 76.0).max(110.0))
                    .show_ui(ui, |ui| {
                        for (index, name) in sheet_names.iter().enumerate() {
                            if ui.selectable_label(index == sheet_idx, name).clicked() {
                                self.switch_sheet(side, index);
                            }
                        }
                    });
                    if sheet_names.len() > 1 {
                        ui.label(
                            egui::RichText::new(format!("共 {} 个", sheet_names.len()))
                                .size(12.0)
                                .color(Self::muted()),
                        );
                    }
                });
                ui.add_space(13.0);
                ui.label(
                    egui::RichText::new(match side {
                        Side::Left => "✓ 将保留主表全部行",
                        Side::Right => "✓ 可从匹配表带出所需要的行",
                    })
                    .size(12.0)
                    .color(Self::teal()),
                );
            } else {
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("选择一个工作簿后，可在这里切换工作表")
                        .size(13.0)
                        .color(Self::muted()),
                );
            }

            if let Some(error) = &error {
                ui.add_space(8.0);
                ui.colored_label(Self::amber(), format!("⚠ {error}"));
            }
        });
    }

    // ---------- 第二步:连接配置 ----------

    fn ui_step_config(&mut self, ui: &mut egui::Ui) {
        let status = if self.sources_ready() {
            Some("配置完整")
        } else {
            None
        };
        Self::work_panel(
            ui,
            "02",
            "连接配置",
            "告诉 ExcelLookup 如何对齐两张表",
            status,
            |ui| self.ui_config_body(ui),
        );
    }

    fn ui_config_body(&mut self, ui: &mut egui::Ui) {
        if !self.sources_ready() {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(egui::RichText::new("请先加载 A、B 两个数据源").size(16.0).strong());
                ui.add_space(12.0);
                if Self::primary_button(ui, "返回数据源", 120.0, true).clicked() {
                    self.go_to_step(WorkflowStep::Sources);
                }
            });
            return;
        }

        let left_headers = self.left.cur_headers();
        let right_headers = self.right.cur_headers();
        Self::sub_panel(ui, |ui| {
            if ui.available_width() >= 700.0 {
                ui.columns(3, |cols| {
                    cols[0].vertical(|ui| {
                        ui.label(
                            egui::RichText::new("A 匹配列")
                                .size(13.0)
                                .strong()
                                .color(Self::muted()),
                        );
                        ui.add_space(7.0);
                        Self::col_combo(ui, "workflow_a_key", &left_headers, &mut self.left_key_col);
                    });
                    cols[1].vertical(|ui| {
                        ui.label(
                            egui::RichText::new("连接类型")
                                .size(13.0)
                                .strong()
                                .color(Self::muted()),
                        );
                        ui.add_space(7.0);
                        egui::ComboBox::from_id_salt("workflow_join_type")
                            .selected_text(Self::join_type_short(self.join_type))
                            .width(ui.available_width())
                            .show_ui(ui, |ui| {
                                for join_type in JoinType::all() {
                                    ui.selectable_value(
                                        &mut self.join_type,
                                        join_type,
                                        join_type.label(),
                                    );
                                }
                            });
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new(self.join_type.hint())
                                .size(12.0)
                                .color(Self::soft()),
                        );
                    });
                    cols[2].vertical(|ui| {
                        ui.label(
                            egui::RichText::new("B 匹配列")
                                .size(13.0)
                                .strong()
                                .color(Self::muted()),
                        );
                        ui.add_space(7.0);
                        Self::col_combo(ui, "workflow_b_key", &right_headers, &mut self.right_key_col);
                        // 键列不允许作为输出列:改了键,同步从输出列中剔除
                        self.right_pick_cols.retain(|&c| Some(c) != self.right_key_col);
                    });
                });
            } else {
                ui.label(egui::RichText::new("A 匹配列").strong().color(Self::muted()));
                Self::col_combo(ui, "workflow_a_key_small", &left_headers, &mut self.left_key_col);
                ui.add_space(10.0);
                ui.label(egui::RichText::new("连接类型").strong().color(Self::muted()));
                egui::ComboBox::from_id_salt("workflow_join_type_small")
                    .selected_text(self.join_type.label())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for join_type in JoinType::all() {
                            ui.selectable_value(&mut self.join_type, join_type, join_type.label());
                        }
                    });
                ui.label(
                    egui::RichText::new(self.join_type.hint())
                        .size(12.0)
                        .color(Self::soft()),
                );
                ui.add_space(10.0);
                ui.label(egui::RichText::new("B 匹配列").strong().color(Self::muted()));
                Self::col_combo(ui, "workflow_b_key_small", &right_headers, &mut self.right_key_col);
                // 键列不允许作为输出列:改了键,同步从输出列中剔除
                self.right_pick_cols.retain(|&c| Some(c) != self.right_key_col);
            }
        });

        ui.add_space(13.0);
        Self::sub_panel(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("B 输出列").size(13.0).strong().color(Self::muted()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("选择需要带入结果的字段")
                            .size(12.0)
                            .color(Self::soft()),
                    );
                });
            });
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                let right_key = self.right_key_col;
                let mut key_unset = false;
                if right_key.is_none() {
                    key_unset = true;
                    ui.label(
                        egui::RichText::new("请先选择 B 匹配列，再勾选输出字段")
                            .size(12.0)
                            .color(Self::amber()),
                    );
                }
                for (index, header) in right_headers.iter().enumerate() {
                    if key_unset || Some(index) == right_key {
                        continue;
                    }
                    let selected = self.right_pick_cols.contains(&index);
                    if Self::toggle_chip(ui, header, selected).clicked() {
                        if selected {
                            self.right_pick_cols.retain(|&column| column != index);
                        } else {
                            self.right_pick_cols.push(index);
                        }
                    }
                }
                if right_key.is_some() && self.right_pick_cols.is_empty() {
                    ui.label(
                        egui::RichText::new("未选择字段，结果将只有 A 的列")
                            .size(12.0)
                            .color(Self::soft()),
                    );
                }
            });
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                Self::toggle_switch(ui, &mut self.normalize_keys, "键宽松匹配");
                ui.label(
                    egui::RichText::new("数字/文本互认，忽略首尾空格")
                        .size(12.0)
                        .color(Self::soft()),
                );
                ui.add_space(15.0);
                Self::toggle_switch(ui, &mut self.bracket_fold, "括号归一化");
                ui.label(
                    egui::RichText::new("中文（）与英文()互认")
                        .size(12.0)
                        .color(Self::soft()),
                );
            });
        });

        Self::action_row(ui, "配置会保留，可随时返回调整", |ui| {
            let keys_ready = self.left_key_col.is_some() && self.right_key_col.is_some();
            if Self::primary_button(ui, "执行连接并查看结果  →", 180.0, keys_ready).clicked() {
                self.run_join();
            }
            if Self::secondary_button(ui, "上一步", 72.0).clicked() {
                self.go_to_step(WorkflowStep::Sources);
            }
            if self.result.is_some()
                && Self::secondary_button(ui, "清空结果", 80.0).clicked()
            {
                self.clear_result();
            }
        });
    }

    fn join_type_short(join_type: JoinType) -> &'static str {
        match join_type {
            JoinType::Left => "左连接",
            JoinType::Inner => "内连接",
            JoinType::Right => "右连接",
            JoinType::Full => "全连接",
        }
    }

    fn col_combo(ui: &mut egui::Ui, id: &str, headers: &[String], sel: &mut Option<usize>) {
        if headers.is_empty() {
            ui.add_enabled(false, egui::Button::new("—"));
            return;
        }
        let selected = sel.and_then(|i| headers.get(i)).cloned();
        egui::ComboBox::from_id_salt(id)
            .selected_text(selected.unwrap_or_else(|| "(请选择匹配列)".to_owned()))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (index, name) in headers.iter().enumerate() {
                    ui.selectable_value(sel, Some(index), name);
                }
            });
    }

    fn toggle_chip(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
        let fill = if selected {
            Self::blue_soft()
        } else {
            Self::white()
        };
        let stroke = if selected {
            Color32::from_rgb(197, 216, 243)
        } else {
            Self::line_strong()
        };
        let text = if selected {
            egui::RichText::new(format!("• {label}")).size(13.0).color(Self::blue())
        } else {
            egui::RichText::new(format!("＋ {label}")).size(13.0).color(Self::muted())
        };
        ui.add(
            egui::Button::new(text)
                .min_size(egui::vec2(0.0, 29.0))
                .fill(fill)
                .stroke(Stroke::new(1.0, stroke))
                .corner_radius(CornerRadius::ZERO),
        )
    }

    fn toggle_switch(ui: &mut egui::Ui, value: &mut bool, label: &str) {
        ui.horizontal(|ui| {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 18.0), egui::Sense::click());
            if response.clicked() {
                *value = !*value;
            }
            let fill = if *value { Self::teal() } else { Self::line_strong() };
            ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
            let knob = if *value {
                egui::pos2(rect.right() - 8.0, rect.center().y)
            } else {
                egui::pos2(rect.left() + 8.0, rect.center().y)
            };
            ui.painter().circle_filled(knob, 6.0, Color32::WHITE);
            ui.label(egui::RichText::new(label).size(13.0).color(Self::muted()));
        });
    }

    // ---------- 第三步:结果预览 ----------

    fn ui_step_result(&mut self, ui: &mut egui::Ui) {
        let status = if self.result_ready() {
            Some("连接完成")
        } else {
            None
        };
        Self::work_panel(
            ui,
            "03",
            "连接结果",
            "预览确认后再导出",
            status,
            |ui| self.ui_result_body(ui),
        );
    }

    fn ui_result_body(&mut self, ui: &mut egui::Ui) {
        let Some(result) = &self.result else {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.label(egui::RichText::new("执行连接后，结果会显示在这里").size(16.0).color(Self::muted()));
                ui.add_space(12.0);
                if Self::primary_button(ui, "返回连接配置", 130.0, true).clicked() {
                    self.go_to_step(WorkflowStep::Configure);
                }
            });
            return;
        };
        if let Some(error) = &result.err {
            ui.colored_label(Self::amber(), format!("⚠ {error}"));
            Self::action_row(ui, "请返回连接配置检查数据", |ui| {
                if Self::secondary_button(ui, "返回配置", 90.0).clicked() {
                    self.go_to_step(WorkflowStep::Configure);
                }
            });
            return;
        }

        let left_total = result.left_total;
        let left_matched = result.left_matched;
        let left_unmatched = left_total.saturating_sub(left_matched);
        let right_total = result.right_total;
        let right_matched = result.right_matched_rows;
        let out_rows = result.out_rows;
        let join_label = result.join_type.label();

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(join_label).size(13.0).strong().color(Self::blue()));
            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "A：{left_total} 行 · B：{right_total} 行 · B 命中 {right_matched} 行"
                ))
                .size(12.0)
                .color(Self::muted()),
            );
        });
        ui.add_space(13.0);

        ui.columns(4, |cols| {
            Self::metric_card(&mut cols[0], "A 主表行数", &left_total.to_string(), "全部保留", Self::blue());
            Self::metric_card(
                &mut cols[1],
                "已匹配",
                &left_matched.to_string(),
                &format!(
                    "匹配率 {:.1}%",
                    if left_total == 0 {
                        0.0
                    } else {
                        left_matched as f64 / left_total as f64 * 100.0
                    }
                ),
                Self::teal(),
            );
            Self::metric_card(
                &mut cols[2],
                "未命中",
                &left_unmatched.to_string(),
                "建议检查匹配列",
                Self::amber(),
            );
            Self::metric_card(
                &mut cols[3],
                "结果行数",
                &out_rows.to_string(),
                "含重复键展开",
                Self::muted(),
            );
        });

        ui.add_space(17.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("结果预览").size(15.0).strong().color(Self::ink()));
            ui.label(
                egui::RichText::new(format!(
                    "{} 行 × {} 列",
                    result.table.row_count(),
                    result.table.col_count()
                ))
                .size(12.0)
                .color(Self::muted()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let edit = egui::TextEdit::singleline(&mut self.result_filter)
                    .hint_text("筛选结果")
                    .desired_width(180.0)
                    .font(egui::TextStyle::Small);
                ui.add(edit);
            });
        });
        ui.add_space(8.0);
        self.ui_result_table(ui);

        Self::action_row(ui, "结果已生成，可返回配置调整", |ui| {
            if Self::secondary_button(ui, "返回配置", 88.0).clicked() {
                self.go_to_step(WorkflowStep::Configure);
            }
        });
    }

    fn metric_card(ui: &mut egui::Ui, label: &str, value: &str, note: &str, accent: Color32) {
        Self::card_frame(Self::surface(), Self::line(), 9).show(ui, |ui| {
            ui.set_min_height(70.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).size(12.0).color(Self::muted()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::same(4))
                        .fill(accent.gamma_multiply(0.10))
                        .corner_radius(CornerRadius::ZERO)
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("▦").size(13.0).color(accent));
                        });
                });
            });
            ui.add_space(7.0);
            ui.label(egui::RichText::new(value).size(24.0).strong().color(Self::ink()));
            ui.label(egui::RichText::new(note).size(11.0).color(accent));
        });
    }

    fn ui_result_table(&self, ui: &mut egui::Ui) {
        let Some(result) = &self.result else { return };
        if result.err.is_some() || result.table.col_count() == 0 {
            return;
        }

        let table = &result.table;
        let headers = table.headers.clone();
        let ncols = table.col_count();
        let filter = self.result_filter.trim().to_lowercase();
        let visible_rows: Option<Vec<usize>> = if filter.is_empty() {
            None
        } else {
            Some(
                table
                    .rows
                    .iter()
                    .enumerate()
                    .filter_map(|(index, row)| {
                        let found = row
                            .iter()
                            .any(|cell| cell.display().to_lowercase().contains(&filter));
                        found.then_some(index)
                    })
                    .collect(),
            )
        };
        let row_count = visible_rows
            .as_ref()
            .map(|rows| rows.len())
            .unwrap_or(table.row_count());

        if row_count == 0 {
            ui.label(egui::RichText::new("没有符合条件的行").size(13.0).color(Self::muted()));
            return;
        }

        let text_h = ui.text_style_height(&egui::TextStyle::Body);
        let row_h = (text_h + 7.0).max(22.0);
        let sep_color = Self::line();
        // 预览表允许拖拽列宽（表头分隔线），便于展开长文本列；
        // 命中区仅在表头细线处，代价可忽略。
        let mut builder = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            // 必须有 click/drag 位才有真正 hover 感应（egui 0.36 Sense::hover() 为空）；
            // 行内格子 hover 后由 egui_extras 自动整行高亮（用于长行横向对位）。
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .vscroll(true)
            .max_scroll_height(300.0);
        if ncols >= 2 {
            builder = builder
                .columns(Column::auto().at_least(70.0).clip(true), ncols - 1)
                .column(Column::remainder().at_least(80.0).clip(true));
        } else {
            builder = builder.columns(Column::remainder().at_least(80.0).clip(true), ncols);
        }

        builder
            .header(row_h, |mut header| {
                for name in &headers {
                    header.col(|ui| {
                        ui.label(egui::RichText::new(name).size(13.0).strong().color(Self::muted()));
                    });
                }
            })
            .body(|body| {
                body.rows(row_h, row_count, |mut row| {
                    let index = visible_rows
                        .as_ref()
                        .map(|rows| rows[row.index()])
                        .unwrap_or(row.index());
                    for column in 0..ncols {
                        row.col(|ui| {
                            match table.cell(index, column) {
                                None | Some(CellValue::Empty) => {
                                    ui.label(egui::RichText::new("—").size(13.0).color(Self::soft()));
                                }
                                Some(value) => {
                                    ui.label(
                                        egui::RichText::new(value.display())
                                            .size(13.0)
                                            .color(Self::ink()),
                                    );
                                }
                            }
                            let rect = ui.max_rect();
                            let painter = ui.painter();
                            painter.hline(
                                rect.x_range(),
                                rect.bottom(),
                                Stroke::new(1.0, sep_color),
                            );
                            if column < ncols - 1 {
                                painter.vline(
                                    rect.right(),
                                    rect.y_range(),
                                    Stroke::new(1.0, sep_color),
                                );
                            }
                        });
                    }
                });
            });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("显示前 {} 行 · 可滚动查看完整结果", row_count.min(100)))
                    .size(12.0)
                    .color(Self::soft()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new("大数据表已启用虚拟滚动")
                        .size(12.0)
                        .color(Self::soft()),
                );
            });
        });
    }

    /// 配置 egui 视觉样式:浅色工作区 + 深色流程侧栏。
    fn configure_ui_style(ctx: &egui::Context) {
        let mut visuals = egui::Visuals::light();
        visuals.panel_fill = Self::canvas();
        visuals.window_fill = Self::white();
        visuals.faint_bg_color = Self::surface();
        visuals.extreme_bg_color = Self::white();
        visuals.text_edit_bg_color = Some(Self::white());
        visuals.hyperlink_color = Self::blue();
        visuals.warn_fg_color = Self::amber();
        visuals.error_fg_color = Color32::from_rgb(177, 74, 61);
        visuals.window_corner_radius = CornerRadius::ZERO;
        visuals.menu_corner_radius = CornerRadius::ZERO;
        visuals.window_shadow = Shadow {
            offset: [0, 3],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(18),
        };
        visuals.window_stroke = Stroke::new(1.0, Self::line());
        visuals.widgets.noninteractive.bg_fill = Self::white();
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Self::line());
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Self::ink());
        visuals.widgets.inactive.bg_fill = Self::white();
        visuals.widgets.inactive.weak_bg_fill = Self::white();
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Self::line_strong());
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Self::ink());
        visuals.widgets.hovered.bg_fill = Self::blue_soft();
        visuals.widgets.hovered.weak_bg_fill = Self::blue_soft();
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Self::blue());
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Self::blue());
        visuals.widgets.active.bg_fill = Self::blue_soft();
        visuals.widgets.active.weak_bg_fill = Self::blue_soft();
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, Self::blue());
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, Self::blue());
        visuals.widgets.noninteractive.corner_radius = CornerRadius::ZERO;
        visuals.widgets.inactive.corner_radius = CornerRadius::ZERO;
        visuals.widgets.hovered.corner_radius = CornerRadius::ZERO;
        visuals.widgets.active.corner_radius = CornerRadius::ZERO;
        visuals.widgets.open.corner_radius = CornerRadius::ZERO;
        visuals.selection.bg_fill = Self::blue_soft();
        visuals.selection.stroke = Stroke::new(1.0, Self::blue());
        // Windows 的桌面文字通常更接近像素对齐效果，关闭 egui 的子像素分箱可减少
        // 小字号 Latin 字符的发虚；CJK 字符本身不会启用该模式。
        if cfg!(windows) {
            visuals.text_options.subpixel_binning = false;
        }
        ctx.set_visuals(visuals);

        ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 5.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            style.spacing.interact_size = egui::vec2(32.0, 32.0);
            style.spacing.icon_width = 18.0;
            style.spacing.icon_width_inner = 13.0;
            style.spacing.icon_spacing = 5.0;
            style.spacing.combo_width = 160.0;
            style.spacing.window_margin = egui::Margin::same(10);

            style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(14.0));
            style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(17.0));
            style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(16.0));
            style.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(16.0));
            style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(29.0));
        });
    }
}

/// 安装 CJK 字体(运行时从系统加载,避免二进制膨胀)
fn install_cjk_font(ctx: &egui::Context) {
    use egui::FontDefinitions;

    let mut fonts = FontDefinitions::default();
    // msyh.ttc 的第 1 个 face 是 Microsoft YaHei UI，更接近 Windows 普通桌面控件。
    // 后续字体仅在前一个文件不存在时作为整套界面的回退字体。
    let candidates: &[(&str, u32)] = if cfg!(windows) {
        &[
            ("C:\\Windows\\Fonts\\msyh.ttc", 1),
            ("C:\\Windows\\Fonts\\simhei.ttf", 0),
            ("C:\\Windows\\Fonts\\simsun.ttc", 0),
        ]
    } else {
        &[
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
            ("/usr/share/fonts/wqy-microhei/wqy-microhei.ttc", 0),
        ]
    };
    for &(path, face_index) in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut data = egui::FontData::from_owned(bytes);
            data.index = face_index;
            fonts.font_data.insert(
                "system_ui".to_owned(),
                std::sync::Arc::new(data),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                let family_fonts = fonts.families.entry(family).or_default();
                if cfg!(windows) {
                    // 不再让 Ubuntu-Light/Hack 与中文字体混排，避免字宽、字重和基线不一致。
                    family_fonts.insert(0, "system_ui".to_owned());
                } else {
                    family_fonts.push("system_ui".to_owned());
                }
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}
