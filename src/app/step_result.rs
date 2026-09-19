//! 第三步：结果预览（命中指标、行筛选、导出进度与结果表格）。

use super::*;

impl ExcelLookupApp {
    // ---------- 第三步:结果预览 ----------

    pub(crate) fn ui_step_result(&mut self, ui: &mut egui::Ui) {
        let status = if self.export_active() {
            Some("正在导出")
        } else if self.join_active {
            Some("正在连接")
        } else if self.result_ready() {
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

    pub(crate) fn ui_result_body(&mut self, ui: &mut egui::Ui) {
        self.ui_export_progress(ui);
        let Some(result) = &self.result else {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                if self.join_active {
                    ui.label(
                        egui::RichText::new("正在后台连接两张表…")
                            .size(16.0)
                            .color(Self::blue()),
                    );
                    return;
                }
                ui.label(egui::RichText::new("执行连接后，结果会显示在这里").size(16.0).color(Self::muted()));
                ui.add_space(12.0);
                if Self::primary_button(ui, "返回连接配置", 130.0, true).clicked() {
                    self.go_to_step(WorkflowStep::Configure);
                }
            });
            return;
        };
        if let Some(error) = &result.err {
            // 诊断错误卡片:首行标题,后续分段(空行分隔的【】小节 + 条目)
            // 先拷贝错误文本,避免闭包内再可变借用 self
            let err_text = error.clone();
            let has_diag = err_text.contains("【数据诊断】");
            let mut go_config = false;
            let mut disable_expand = false;
            Self::card_frame(Self::surface(), Self::line(), 16).show(ui, |ui| {
                ui.set_min_width(600.0);
                let mut lines = err_text.lines();
                // 标题行
                if let Some(head) = lines.next() {
                    ui.label(
                        egui::RichText::new(format!("⚠ {head}"))
                            .size(15.0)
                            .strong()
                            .color(Self::amber()),
                    );
                }
                ui.add_space(6.0);
                // 分段渲染:空行分隔小节,【】开头的行作小标题,其余作正文
                for l in lines {
                    let t = l.trim();
                    if t.is_empty() {
                        ui.add_space(8.0);
                        continue;
                    }
                    if t.starts_with("【") {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(t).size(13.0).strong().color(Self::ink()));
                        ui.add_space(2.0);
                    } else {
                        ui.label(
                            egui::RichText::new(t.trim_start_matches("· "))
                                .size(13.0)
                                .color(Self::ink()),
                        );
                    }
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if Self::primary_button(ui, "返回连接配置", 130.0, true).clicked() {
                        go_config = true;
                    }
                    if has_diag && Self::secondary_button(ui, "关闭重复键展开", 150.0).clicked() {
                        disable_expand = true;
                    }
                });
            });
            if go_config {
                self.go_to_step(WorkflowStep::Configure);
            }
            if disable_expand {
                self.expand_dup = false;
                self.go_to_step(WorkflowStep::Configure);
            }
            return;
        }

        let left_total = result.left_total;
        let left_matched = result.left_matched;
        let right_total = result.right_total;
        let right_matched = result.right_matched_rows;
        let out_rows = result.out_rows;
        let join_label = result.join_type.label();
        // row_hit 预计算计数(结果表口径;重复键展开时命中行数可能 > A 命中行数)
        let matched_rows = result.matched_rows;
        let unmatched_rows = result.unmatched_rows;

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
            Self::metric_card(
                &mut cols[0],
                "A 主表行数",
                &left_total.to_string(),
                "全部保留",
                Self::blue(),
                false,
                false,
            );
            let matched_resp = Self::metric_card(
                &mut cols[1],
                "已匹配",
                &matched_rows.to_string(),
                &format!(
                    "A 命中 {left_matched} 行 · 匹配率 {:.1}%",
                    if left_total == 0 {
                        0.0
                    } else {
                        left_matched as f64 / left_total as f64 * 100.0
                    }
                ),
                Self::teal(),
                true,
                self.row_filter == Some(RowFilter::Matched),
            );
            let unmatched_resp = Self::metric_card(
                &mut cols[2],
                "未命中",
                &unmatched_rows.to_string(),
                if self.row_filter == Some(RowFilter::Unmatched) {
                    "再次点击取消筛选"
                } else {
                    "点击仅看未命中行"
                },
                Self::amber(),
                true,
                self.row_filter == Some(RowFilter::Unmatched),
            );
            Self::metric_card(
                &mut cols[3],
                "结果行数",
                &out_rows.to_string(),
                if result.expand_dup {
                    "含重复键展开"
                } else {
                    "重复键只取第一条"
                },
                Self::muted(),
                false,
                false,
            );

            if matched_resp.clicked() {
                self.row_filter = if self.row_filter == Some(RowFilter::Matched) {
                    None
                } else {
                    Some(RowFilter::Matched)
                };
            }
            if unmatched_resp.clicked() {
                self.row_filter = if self.row_filter == Some(RowFilter::Unmatched) {
                    None
                } else {
                    Some(RowFilter::Unmatched)
                };
            }
        });

        ui.add_space(17.0);
        // 按当前筛选显示对应行数(总行数 / 已匹配 / 未命中)
        let shown_total = match self.row_filter {
            Some(RowFilter::Matched) => matched_rows,
            Some(RowFilter::Unmatched) => unmatched_rows,
            None => result.table.row_count(),
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("结果预览").size(15.0).strong().color(Self::ink()));
            ui.label(
                egui::RichText::new(format!("{shown_total} 行 × {} 列", result.table.col_count()))
                    .size(12.0)
                    .color(Self::muted()),
            );
            match self.row_filter {
                Some(RowFilter::Matched) => {
                    ui.label(
                        egui::RichText::new("· 只看已匹配")
                            .size(12.0)
                            .color(Self::teal()),
                    );
                }
                Some(RowFilter::Unmatched) => {
                    ui.label(
                        egui::RichText::new("· 只看未命中")
                            .size(12.0)
                            .color(Self::amber()),
                    );
                }
                None => {}
            }
        });
        ui.add_space(8.0);
        self.ui_result_table(ui);

        Self::action_row(ui, "结果已生成，可返回配置调整", NoteTone::Normal, |ui| {
            if Self::secondary_button(ui, "返回配置", 88.0).clicked() {
                self.go_to_step(WorkflowStep::Configure);
            }
        });
    }

    /// 指标卡;返回整卡可点击的 Response(用于未命中筛选交互)
    /// `active` = 是否处于激活(筛选)态,改变描边/底色提示可再点取消
    pub(crate) fn metric_card(
        ui: &mut egui::Ui,
        label: &str,
        value: &str,
        note: &str,
        accent: Color32,
        clickable: bool,
        active: bool,
    ) -> egui::Response {
        let stroke_color = if active {
            accent
        } else if clickable {
            Self::line_strong()
        } else {
            Self::line()
        };
        let stroke_w = if active { 2.0 } else { 1.0 };
        let fill = if active {
            accent.gamma_multiply(0.08)
        } else {
            Self::surface()
        };
        let inner = Self::card_frame(fill, stroke_color, 9)
            .stroke(Stroke::new(stroke_w, stroke_color))
            .show(ui, |ui| {
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
        let sense = if clickable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        ui.interact(inner.response.rect, ui.id().with(("metric", label)), sense)
            .on_hover_cursor(if clickable {
                egui::CursorIcon::PointingHand
            } else {
                egui::CursorIcon::Default
            })
    }

    pub(crate) fn ui_export_progress(&mut self, ui: &mut egui::Ui) {
        match &self.export_state {
            ExportState::Running(progress) => {
                let (phase_label, fraction, detail) = match progress.phase {
                    ExportPhase::Writing => {
                        let fraction = if progress.total_rows == 0 {
                            0.85
                        } else {
                            (progress.completed_rows as f32 / progress.total_rows as f32 * 0.85)
                                .min(0.85)
                        };
                        (
                            "正在写入数据…",
                            fraction,
                            format!(
                                "已写入 {} / {} 行",
                                progress.completed_rows, progress.total_rows
                            ),
                        )
                    }
                    ExportPhase::Saving => {
                        ("正在压缩工作簿…", 0.92, "大数据量保存需要一些时间".to_owned())
                    }
                };

                Self::card_frame(Self::surface(), Self::line(), 13).show(ui, |ui| {
                    ui.set_min_width(600.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("正在导出")
                                .size(15.0)
                                .strong()
                                .color(Self::blue()),
                        );
                        ui.label(
                            egui::RichText::new(phase_label)
                                .size(13.0)
                                .color(Self::muted()),
                        );
                    });
                    ui.add_space(7.0);
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_width(420.0)
                            .show_percentage()
                            .animate(true),
                    );
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(detail).size(12.0).color(Self::soft()));
                });
                ui.add_space(10.0);
            }
            ExportState::Done(path) => {
                let path = path.clone();
                let location_error = self.export_location_error.clone();
                let mut open_location = false;
                Self::card_frame(Self::surface(), Self::line(), 13).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("导出完成，可打开刚保存的工作簿。")
                                .size(13.0)
                                .color(Self::blue()),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if Self::secondary_button(ui, "打开文件位置", 128.0).clicked() {
                                    open_location = true;
                                }
                            },
                        );
                    });
                    if let Some(error) = &location_error {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(error)
                                .size(12.0)
                                .color(Self::amber()),
                        );
                    }
                });
                if open_location {
                    self.export_location_error = open_export_location(&path).err();
                }
                ui.add_space(10.0);
            }
            ExportState::Idle => {}
        }
    }

    pub(crate) fn ui_result_table(&mut self, ui: &mut egui::Ui) {
        let Some(result) = &self.result else { return };
        if result.err.is_some() || result.table.col_count() == 0 {
            return;
        }

        // Arc 克隆只是引用计数,避免在重建筛选缓存时与 self.result 的借用冲突。
        let table = Arc::clone(&result.table);
        let left_source = Arc::clone(&result.left_source);
        let right_source = Arc::clone(&result.right_source);
        let headers = table.headers.clone();
        let ncols = table.col_count();
        let matched_rows = result.matched_rows;
        let unmatched_rows = result.unmatched_rows;
        let result_id = result.result_id;
        // 行筛选:已匹配/未命中(按行引用派生的命中标志过滤);None=全部
        let row_filter = self.row_filter;
        // 每帧对 500 万行重新收集一遍行号是滚动卡顿的根源,按「结果版本 + 筛选
        // 条件」缓存;全命中/全未命中直接复用全集,不物化行号表。
        let visible_rows: Option<FilteredRows> = match row_filter {
            None => None,
            Some(filter) => {
                let cached = self
                    .filter_cache
                    .as_ref()
                    .filter(|cache| cache.result_id == result_id && cache.filter == filter)
                    .map(|cache| cache.rows.clone());
                Some(match cached {
                    Some(rows) => rows,
                    None => {
                        let rows = filter_rows(&table, filter, matched_rows, unmatched_rows);
                        self.filter_cache = Some(FilterCache {
                            result_id,
                            filter,
                            rows: rows.clone(),
                        });
                        rows
                    }
                })
            }
        };
        let row_count = visible_rows
            .as_ref()
            .map(|rows| rows.len(table.row_count()))
            .unwrap_or(table.row_count());

        if row_count == 0 {
            let msg = match row_filter {
                Some(RowFilter::Matched) => "没有已匹配的行（当前连接类型下全部未命中）",
                Some(RowFilter::Unmatched) => "没有未命中的行（当前连接类型下全部命中）",
                None => "没有符合条件的行",
            };
            ui.label(egui::RichText::new(msg).size(13.0).color(Self::muted()));
            return;
        }

        let text_h = ui.text_style_height(&egui::TextStyle::Body);
        let row_h = (text_h + 7.0).max(22.0);
        let sep_color = Self::line();
        // 横向滚动容器:表格总宽超过视口时可左右滚动查看所有列;
        // Table 内部仍是纵向虚拟滚动(只渲染可见行),性能不受影响。
        egui::ScrollArea::horizontal()
            .id_salt("result_table_hscroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // 每个数据列:initial(初始宽按内容舒适值,可拖拽调宽,文本截断)
                // 相比 auto:auto 会在空间不足时缩到 70;initial 保持稳定宽度,
                // 列数多时通过外层横滚访问,不会被压扁。
                let col_initial = |i: usize| {
                    let name_w = headers[i].chars().count() as f32 * 14.0 + 20.0;
                    name_w.clamp(90.0, 260.0)
                };
                let mut builder = TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .sense(egui::Sense::click())
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .vscroll(true)
                    .max_scroll_height(300.0);
                for i in 0..ncols {
                    let w = col_initial(i);
                    builder = builder.column(Column::initial(w).at_least(70.0).clip(true));
                }

                builder
                    .header(row_h, |mut header| {
                        for name in &headers {
                            header.col(|ui| {
                                ui.label(
                                    egui::RichText::new(name)
                                        .size(13.0)
                                        .strong()
                                        .color(Self::muted()),
                                );
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(row_h, row_count, |mut row| {
                            let index = visible_rows
                                .as_ref()
                                .map(|rows| rows.index(row.index()))
                                .unwrap_or(row.index());
                            for column in 0..ncols {
                                row.col(|ui| {
                                    // 列 clip 时单元格 wrap_mode=Truncate,Label 默认在文本
                                    // 被截断(elided)时自动弹全文 tooltip,无需手动添加。
                                    match table.cell(&left_source, &right_source, index, column) {
                                        None | Some(CellValue::Empty) => {
                                            ui.label(
                                                egui::RichText::new("—")
                                                    .size(13.0)
                                                    .color(Self::soft()),
                                            );
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
                    egui::RichText::new("列宽可拖拽 · 横向可滚动")
                        .size(12.0)
                        .color(Self::soft()),
                );
            });
        });
    }
}
