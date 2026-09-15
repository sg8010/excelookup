//! 第二步：连接配置（匹配列、连接类型、输出列与匹配选项）。

use super::*;

impl ExcelLookupApp {
    // ---------- 第二步:连接配置 ----------

    pub(crate) fn ui_step_config(&mut self, ui: &mut egui::Ui) {
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

    pub(crate) fn ui_config_body(&mut self, ui: &mut egui::Ui) {
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
                        .selected_text(self.join_type.label())
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
                ui.add_space(15.0);
                Self::toggle_switch(ui, &mut self.expand_dup, "重复键展开");
                if self.expand_dup {
                    ui.label(
                        egui::RichText::new("B 同键多行全部带出")
                            .size(12.0)
                            .color(Self::soft()),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("B 同键多行只取第一条（VLOOKUP 风格）")
                            .size(12.0)
                            .color(Self::amber()),
                    );
                }
            });
        });

        Self::action_row(ui, "配置会保留，可随时返回调整", |ui| {
            let keys_ready = self.left_key_col.is_some() && self.right_key_col.is_some();
            // join 期间置灰:世代号只保证旧结果不落回,不会停下旧线程的计算。
            let busy = self.join_active;
            let label = if busy {
                "正在连接…"
            } else {
                "执行连接并查看结果  →"
            };
            if Self::primary_button(ui, label, 180.0, keys_ready && !busy).clicked() {
                self.run_join(ui.ctx().clone());
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

    pub(crate) fn col_combo(ui: &mut egui::Ui, id: &str, headers: &[String], sel: &mut Option<usize>) {
        if headers.is_empty() {
            ui.add_enabled(false, egui::Button::new("—"));
            return;
        }
        let selected = sel.and_then(|i| headers.get(i)).cloned();
        egui::ComboBox::from_id_salt(id)
            .selected_text(selected.unwrap_or_else(|| "(请选择匹配列)".to_owned()))
            .width(ui.available_width())
            .truncate()
            .show_ui(ui, |ui| {
                for (index, name) in headers.iter().enumerate() {
                    ui.selectable_value(sel, Some(index), name);
                }
            });
    }

    pub(crate) fn toggle_chip(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
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

    pub(crate) fn toggle_switch(ui: &mut egui::Ui, value: &mut bool, label: &str) {
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
}
