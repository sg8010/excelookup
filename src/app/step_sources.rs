//! 第一步：数据源（选择工作簿、切换工作表、指定列名行）。

use super::*;

impl ExcelLookupApp {
    // ---------- 第一步:数据源 ----------

    pub(crate) fn ui_step_sources(&mut self, ui: &mut egui::Ui) {
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

    pub(crate) fn ui_sources_body(&mut self, ui: &mut egui::Ui) {
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

                let any_loading = self.load_active[0] || self.load_active[1];
                let resp = ui.interact(
                    button_rect,
                    ui.id().with("swap_ab"),
                    if any_loading {
                        egui::Sense::hover()
                    } else {
                        egui::Sense::click()
                    },
                );
                if !any_loading && resp.clicked() {
                    self.pending_swap = true;
                }
                if any_loading {
                    resp.on_hover_text("加载完成后再对调");
                } else {
                    resp.on_hover_text("对调 A/B 表");
                }
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
        Self::action_row(ui, "文件只在本机读取，不会上传", NoteTone::Normal, |ui| {
            if Self::primary_button(ui, "下一步：配置连接  →", 176.0, can_next).clicked() {
                self.go_to_step(WorkflowStep::Configure);
            }
            if Self::secondary_button(ui, "清空数据源", 104.0).clicked() {
                self.clear_sources();
            }
        });
    }

    pub(crate) fn ui_source_card(&mut self, ui: &mut egui::Ui, side: Side) {
        let loading = self.load_active[side.index()];
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
                src.sheet_names(),
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
                Self::letter_badge(ui, side.letter(), accent);
                ui.add_space(1.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(role.0).size(16.0).strong().color(Self::ink()));
                        ui.label(egui::RichText::new(role.1).size(13.0).color(Self::muted()));
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if loading {
                        Self::status_badge(ui, "加载中…");
                    } else if loaded && error.is_none() {
                        Self::status_badge(ui, "已加载");
                    }
                });
            });
            ui.add_space(19.0);

            let file_label = if loading {
                "正在后台读取…".to_owned()
            } else if loaded {
                format!("▣  {label}")
            } else {
                "选择工作簿".to_owned()
            };
            let button_width = ui.available_width();
            ui.add_enabled_ui(!loading, |ui| {
                if ui
                    .add_sized(
                        [button_width, 39.0],
                        egui::Button::new(
                            egui::RichText::new(file_label).strong().color(Self::ink()),
                        )
                        .fill(Self::white())
                        .stroke(Stroke::new(1.0, Self::line_strong()))
                        .corner_radius(CornerRadius::ZERO),
                    )
                    .clicked()
                {
                    self.pick_and_load(side);
                }
            });

            if loading {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("大文件解析中,界面保持可操作")
                        .size(12.0)
                        .color(Self::muted()),
                );
            }

            if loaded && !loading {
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
                    .truncate()
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
                ui.add_space(11.0);
                self.ui_header_row_picker(ui, side);
                ui.add_space(13.0);
                ui.label(
                    egui::RichText::new(match side {
                        Side::Left => "✓ 将保留主表全部行",
                        Side::Right => "✓ 可从匹配表带出所需要的行",
                    })
                    .size(12.0)
                    .color(Self::teal()),
                );
            } else if !loading {
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("选择一个工作簿后，可在这里切换工作表")
                        .size(13.0)
                        .color(Self::muted()),
                );
            }

            if let Some(error) = &error {
                if !loading {
                    ui.add_space(8.0);
                    ui.colored_label(Self::amber(), format!("⚠ {error}"));
                }
            }
        });
    }

    /// 列名行选择器:首行是合并大标题 / 多行表头时,把列名行指到真正的列名那一行。
    /// 改变即用同一文件重新读取(后台线程,界面不阻塞)。
    pub(crate) fn ui_header_row_picker(&mut self, ui: &mut egui::Ui, side: Side) {
        let (options, current, fell_back) = {
            let src = match side {
                Side::Left => &self.left,
                Side::Right => &self.right,
            };
            let Some(sheet) = src.cur_sheet() else { return };
            // 该工作表自己的选择(未选过 = 自动)
            let current = src.header_rows.get(src.sheet_idx).copied().flatten();
            (
                Self::header_row_options(
                    &sheet.preview,
                    sheet.first_row_number,
                    sheet.auto_header_row,
                    current,
                ),
                current,
                // 只有"明确指定了某行但没被采纳"才算回退;自动不算
                current.is_some_and(|row| sheet.used_header_row != Some(row)),
            )
        };
        let mut picked = current;
        let selected_text = options
            .iter()
            .find(|(value, _)| *value == current)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| "自动".to_owned());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("列名行").size(13.0).color(Self::soft()));
            ui.add_space(8.0);
            egui::ComboBox::from_id_salt(match side {
                Side::Left => "workflow_left_header_row",
                Side::Right => "workflow_right_header_row",
            })
            .selected_text(selected_text)
            .width((ui.available_width() - 76.0).max(110.0))
            // 预览文本可能很长;截断选中项,不能让 ComboBox 的最小宽度撑大三列布局。
            .truncate()
            .show_ui(ui, |ui| {
                for (value, label) in &options {
                    ui.selectable_value(&mut picked, *value, label);
                }
            });
        });
        if fell_back {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("⚠ 该工作表行数不足，已回退为自动识别列名行")
                    .size(12.0)
                    .color(Self::amber()),
            );
        }
        if picked == current {
            return;
        }
        // 只改当前工作表的选择;优先复用顶部候选行缓存,否则只重读当前工作表。
        let (path, mut header_rows) = match side {
            Side::Left => (self.left.path.clone(), self.left.header_rows.clone()),
            Side::Right => (self.right.path.clone(), self.right.header_rows.clone()),
        };
        let idx = match side {
            Side::Left => self.left.sheet_idx,
            Side::Right => self.right.sheet_idx,
        };
        if header_rows.len() <= idx {
            header_rows.resize(idx + 1, None);
        }
        header_rows[idx] = picked;
        if let Some(path) = path {
            self.start_sheet_reload(side, path, header_rows, idx, ui.ctx().clone());
        }
    }

    /// 列名行候选:自动 + 顶部原始行(带内容预览)。
    /// `first_row` = 已用区域首行的 Excel 行号(1-based);`current` 落在预览窗口外时也补一项。
    pub(crate) fn header_row_options(
        preview: &[Vec<String>],
        first_row: usize,
        auto_row: Option<usize>,
        current: Option<usize>,
    ) -> Vec<(Option<usize>, String)> {
        let mut out = vec![(
            None,
            match auto_row {
                Some(row) => format!("自动（第 {} 行）", first_row + row),
                None => "自动".to_owned(),
            },
        )];
        for (index, row) in preview.iter().enumerate() {
            let text = row
                .iter()
                .map(|cell| cell.trim())
                .filter(|cell| !cell.is_empty())
                .collect::<Vec<_>>()
                .join(" | ");
            let text: String = text.chars().take(24).collect();
            let text = if text.is_empty() {
                "（空行）".to_owned()
            } else {
                text
            };
            out.push((Some(index), format!("第 {} 行：{}", first_row + index, text)));
        }
        if let Some(row) = current {
            if !out.iter().any(|(value, _)| *value == Some(row)) {
                out.push((Some(row), format!("第 {} 行", first_row + row)));
            }
        }
        out
    }

    pub(crate) fn headers_from_preview(row: &[String], width: usize) -> Vec<String> {
        let mut headers: Vec<String> = row.iter().take(width).cloned().collect();
        headers.resize(width, String::new());
        let mut seen = std::collections::HashMap::new();
        for (index, header) in headers.iter_mut().enumerate() {
            if header.trim().is_empty() {
                *header = format!("列{}", index + 1);
            }
            let count = seen.entry(header.clone()).or_insert(0usize);
            if *count > 0 {
                *header = format!("{}_{}", header, *count + 1);
            }
            *count += 1;
        }
        headers
    }
}
