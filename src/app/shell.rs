//! 外壳与工作流：侧栏、顶栏、页首与上下文条。

use super::*;

impl ExcelLookupApp {
    // ---------- 外壳与工作流 ----------

    pub(crate) fn ui_sidebar(&mut self, ui: &mut egui::Ui) {
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

    pub(crate) fn brand_mark(ui: &mut egui::Ui) {
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

    pub(crate) fn ui_workflow_step(&mut self, ui: &mut egui::Ui, step: WorkflowStep) {
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
                if self.join_active {
                    ("结果预览", "正在连接…")
                } else if self.result_ready() {
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

    pub(crate) fn ui_topbar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("连接工作台").strong().color(Self::ink()));
            ui.label(egui::RichText::new("/").color(Self::soft()));
            ui.label(egui::RichText::new("新建连接").color(Self::muted()));
        });
    }

    pub(crate) fn ui_workspace(&mut self, ui: &mut egui::Ui) {
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

    pub(crate) fn ui_page_heading(&mut self, ui: &mut egui::Ui) {
        let export_active = self.export_active();
        let show_export = self.step == WorkflowStep::Result && self.result_ready() && !export_active;
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
                if export_active {
                    ui.label(
                        egui::RichText::new("正在导出")
                            .size(13.0)
                            .strong()
                            .color(Self::blue()),
                    );
                } else if show_export {
                    if Self::green_button(ui, "导出结果  ↓", 136.0).clicked() {
                        self.pending_dialog = Some(DialogRequest::Save);
                    }
                }
            });
        });
    }

    pub(crate) fn ui_context_strip(&mut self, ui: &mut egui::Ui) {
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

    pub(crate) fn source_context_data(&self, side: Side) -> (String, String, usize, usize) {
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

    pub(crate) fn context_source(
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
                side.letter(),
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
}
