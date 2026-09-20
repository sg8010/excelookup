//! 外壳与工作流：浅色侧栏、页首与上下文条。

use super::*;

/// 侧栏内边距(左/右/上/下)。
pub(crate) const SIDEBAR_MARGIN: egui::Margin = egui::Margin {
    left: 10,
    right: 10,
    top: 20,
    bottom: 14,
};

impl ExcelLookupApp {
    // ---------- 外壳与工作流 ----------

    pub(crate) fn ui_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(ui.available_width());
        // 品牌区:图标与导航条目左对齐(整体右移 12px)。
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            Self::brand_mark(ui);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("ExcelLookup")
                        .size(17.0)
                        .strong()
                        .color(Self::ink()),
                );
                ui.label(
                    egui::RichText::new("双表连接工具")
                        .size(13.0)
                        .color(Self::muted()),
                );
            });
        });

        ui.add_space(22.0);
        ui.label(
            egui::RichText::new("工作流程")
                .size(12.0)
                .strong()
                .color(Self::soft()),
        );
        ui.add_space(8.0);

        self.ui_workflow_step(ui, WorkflowStep::Sources);
        ui.add_space(4.0);
        self.ui_workflow_step(ui, WorkflowStep::Configure);
        ui.add_space(4.0);
        self.ui_workflow_step(ui, WorkflowStep::Result);

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(format!(
                    "v{}",
                    option_env!("EXCELOOKUP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
                ))
                .size(13.0)
                .color(Self::soft()),
            );
        });
    }

    /// 侧栏右边的 1px 分隔线(画满整个侧栏高度)。
    ///
    /// 面板 Frame 的 stroke 只能四边同画,单边分隔线得手绘;
    /// `max_rect + margin` 还原出面板完整矩形。
    pub(crate) fn paint_sidebar_border(ui: &mut egui::Ui) {
        let panel = ui.max_rect() + SIDEBAR_MARGIN;
        ui.painter().with_clip_rect(panel).vline(
            panel.right() - 0.5,
            panel.y_range(),
            Stroke::new(1.0, Self::line()),
        );
    }

    pub(crate) fn brand_mark(ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(8), Self::blue());
        let left = rect.left() + 10.0;
        let right = rect.right() - 10.0;
        let top = rect.top() + 10.0;
        let bottom = rect.bottom() - 10.0;
        let mid = rect.center().y;
        let mark = Stroke::new(1.6, Color32::WHITE);
        painter.line_segment([egui::pos2(left, top), egui::pos2(right, top)], mark);
        painter.line_segment([egui::pos2(left, bottom), egui::pos2(right, bottom)], mark);
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

    /// 侧栏步骤条目:状态圆 + 「标题 + 提示」两行文案;active = 浅蓝底,
    /// 有在途后台任务时右侧画 3px 蓝点。
    pub(crate) fn ui_workflow_step(&mut self, ui: &mut egui::Ui, step: WorkflowStep) {
        let active = self.step == step;
        let done = step.number() < self.step.number();
        let enabled = self.can_enter_step(step);
        let inflight = self.step_inflight(step);
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

        let sense = if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), Self::NAV_ITEM_HEIGHT),
            sense,
        );
        if ui.is_rect_visible(rect) {
            let fill = if active {
                Self::blue_soft()
            } else if enabled && response.hovered() {
                Self::nav_hover()
            } else {
                Color32::TRANSPARENT
            };
            if fill != Color32::TRANSPARENT {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(Self::NAV_RADIUS), fill);
            }
            // 左侧 16px 状态圆:done=绿底白勾、active=蓝底+外圈浅蓝环、其余=描边。
            let center = egui::pos2(rect.left() + 12.0 + 8.0, rect.center().y);
            let painter = ui.painter();
            if done {
                painter.circle_filled(center, 7.5, Self::teal());
                let check = Stroke::new(1.5, Color32::WHITE);
                painter.line_segment(
                    [
                        egui::pos2(center.x - 3.0, center.y + 0.3),
                        egui::pos2(center.x - 0.6, center.y + 2.6),
                    ],
                    check,
                );
                painter.line_segment(
                    [
                        egui::pos2(center.x - 0.6, center.y + 2.6),
                        egui::pos2(center.x + 3.4, center.y - 2.6),
                    ],
                    check,
                );
            } else if active {
                painter.circle_filled(center, 7.5, Self::blue());
                painter.circle_stroke(center, 9.0, Stroke::new(3.0, Self::accent_border()));
            } else {
                painter.circle_stroke(center, 7.5, Stroke::new(1.0, Self::line_strong()));
            }
            let text_x = center.x + 8.0 + 8.0;
            painter.text(
                egui::pos2(text_x, center.y - 9.0),
                egui::Align2::LEFT_CENTER,
                title,
                egui::FontId::proportional(15.0),
                if enabled { Self::ink() } else { Self::soft() },
            );
            painter.text(
                egui::pos2(text_x, center.y + 10.0),
                egui::Align2::LEFT_CENTER,
                hint,
                egui::FontId::proportional(13.0),
                if active { Self::blue() } else { Self::soft() },
            );
            if inflight {
                painter.circle_filled(egui::pos2(rect.right() - 14.0, center.y), 3.0, Self::blue());
            }
        }
        if enabled && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if enabled && response.clicked() {
            self.go_to_step(step);
        }
    }

    /// 该步骤是否有在途后台任务(侧栏状态点的语义)。
    pub(crate) fn step_inflight(&self, step: WorkflowStep) -> bool {
        match step {
            WorkflowStep::Sources => self.load_active[0] || self.load_active[1],
            WorkflowStep::Configure => false,
            WorkflowStep::Result => self.join_active || self.export_active(),
        }
    }

    pub(crate) fn ui_workspace(&mut self, ui: &mut egui::Ui) {
        self.ui_page_heading(ui);
        ui.add_space(Self::HEADER_TO_CONTENT);

        if self.step != WorkflowStep::Sources && self.sources_ready() {
            self.ui_context_strip(ui);
            ui.add_space(Self::SECTION_GAP);
        }

        match self.step {
            WorkflowStep::Sources => self.ui_step_sources(ui),
            WorkflowStep::Configure => self.ui_step_config(ui),
            WorkflowStep::Result => self.ui_step_result(ui),
        }
    }

    /// 页首:左竖排标题 + 说明;右侧放页面级操作(结果页的「导出结果」)。
    pub(crate) fn ui_page_heading(&mut self, ui: &mut egui::Ui) {
        let export_active = self.export_active();
        let show_export =
            self.step == WorkflowStep::Result && self.result_ready() && !export_active;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(self.step.title())
                        .size(28.0)
                        .strong()
                        .color(Self::ink()),
                );
                ui.add_space(Self::TITLE_TO_DESC);
                ui.label(
                    egui::RichText::new(self.step.description())
                        .size(15.0)
                        .color(Self::muted()),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if export_active {
                    Self::status_badge(
                        ui,
                        Some(Icon::Loader),
                        "正在导出",
                        Self::blue_soft(),
                        Self::blue(),
                    );
                } else if show_export
                    && Self::primary_button(ui, Some(Icon::External), "导出结果", 120.0, true)
                        .clicked()
                {
                    self.pending_dialog = Some(DialogRequest::Save);
                }
            });
        });
    }

    pub(crate) fn ui_context_strip(&mut self, ui: &mut egui::Ui) {
        let left = self.source_context_data(Side::Left);
        let right = self.source_context_data(Side::Right);
        Self::card_frame().show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::same(13))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        Self::context_source(ui, Side::Left, &left.0, &left.1, left.2, left.3);
                        ui.separator();
                        Self::context_source(ui, Side::Right, &right.0, &right.1, right.2, right.3);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if Self::secondary_button(ui, None, "更换数据源", 100.0, true).clicked()
                            {
                                self.go_to_step(WorkflowStep::Sources);
                            }
                        });
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
                ui.label(
                    egui::RichText::new(label)
                        .size(13.0)
                        .strong()
                        .color(Self::ink()),
                );
                ui.label(egui::RichText::new(info).size(13.0).color(Self::muted()));
            });
        });
    }
}
