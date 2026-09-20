//! 视觉主题：色板、通用控件与全局样式。

use super::*;

/// 底部说明区文案的配色。
///
/// 用枚举而不是 `bool`,避免 `action_row(ui, note, false, |ui| ...)` 这种在调用点
/// 看不出含义(且容易传反)的布尔参数。
pub(crate) enum NoteTone {
    /// 常规提示(绿色)。
    Normal,
    /// 状态警示:当前配置还不能执行(琥珀色)。
    Warn,
}

/// 简单线条图标(16~18px、约 1.5px stroke;颜色跟随所在控件文本色)。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Doc,
    DocPlus,
    CheckCircle,
    Warn,
    Loader,
    External,
}

/// 按钮三态配色。
#[derive(Clone, Copy)]
struct BtnLook {
    fill: Color32,
    stroke: Stroke,
    text: Color32,
}

impl ExcelLookupApp {
    // ---------- Design tokens:颜色 ----------

    pub(crate) fn canvas() -> Color32 {
        Color32::from_rgb(0xF5, 0xF7, 0xFA)
    }

    pub(crate) fn white() -> Color32 {
        Color32::WHITE
    }

    pub(crate) fn surface() -> Color32 {
        Color32::WHITE
    }

    pub(crate) fn surface_subtle() -> Color32 {
        Color32::from_rgb(0xFA, 0xFB, 0xFC)
    }

    pub(crate) fn line() -> Color32 {
        Color32::from_rgb(0xE3, 0xE8, 0xEF)
    }

    pub(crate) fn line_strong() -> Color32 {
        Color32::from_rgb(0xD0, 0xD7, 0xE2)
    }

    pub(crate) fn divider() -> Color32 {
        Color32::from_rgb(0xE9, 0xED, 0xF3)
    }

    pub(crate) fn ink() -> Color32 {
        Color32::from_rgb(0x17, 0x20, 0x33)
    }

    pub(crate) fn muted() -> Color32 {
        Color32::from_rgb(0x66, 0x70, 0x85)
    }

    pub(crate) fn soft() -> Color32 {
        Color32::from_rgb(0x98, 0xA2, 0xB3)
    }

    pub(crate) fn text_disabled() -> Color32 {
        Color32::from_rgb(0xB8, 0xC0, 0xCC)
    }

    /// 主色(蓝)。
    pub(crate) fn blue() -> Color32 {
        Color32::from_rgb(0x16, 0x77, 0xFF)
    }

    pub(crate) fn accent_hover() -> Color32 {
        Color32::from_rgb(0x0F, 0x6D, 0xE8)
    }

    pub(crate) fn blue_soft() -> Color32 {
        Color32::from_rgb(0xEA, 0xF3, 0xFF)
    }

    pub(crate) fn accent_border() -> Color32 {
        Color32::from_rgb(0xBF, 0xD8, 0xFF)
    }

    /// 成功/完成(绿;B 侧强调色沿用)。
    pub(crate) fn teal() -> Color32 {
        Color32::from_rgb(0x16, 0xA3, 0x4A)
    }

    pub(crate) fn teal_soft() -> Color32 {
        Color32::from_rgb(0xEA, 0xF8, 0xEF)
    }

    /// 警示(琥珀)。
    pub(crate) fn amber() -> Color32 {
        Color32::from_rgb(0xD9, 0x77, 0x06)
    }

    pub(crate) fn warning_soft() -> Color32 {
        Color32::from_rgb(0xFF, 0xF6, 0xE8)
    }

    /// 错误红(全局 error 文字色)。
    pub(crate) fn danger() -> Color32 {
        Color32::from_rgb(0xE5, 0x48, 0x4D)
    }

    /// 侧栏条目 hover 底色。
    pub(crate) fn nav_hover() -> Color32 {
        Color32::from_rgb(0xF3, 0xF7, 0xFC)
    }

    /// 禁用控件底色。
    pub(crate) fn disabled_bg() -> Color32 {
        Color32::from_rgb(0xEE, 0xF1, 0xF5)
    }

    /// 进度条/开关轨道色。
    pub(crate) fn track() -> Color32 {
        Color32::from_rgb(0xEA, 0xEE, 0xF4)
    }

    // ---------- Design tokens:尺寸/圆角/间距 ----------

    pub(crate) const CARD_RADIUS: u8 = 8;
    pub(crate) const BUTTON_RADIUS: u8 = 6;
    pub(crate) const INPUT_RADIUS: u8 = 6;
    pub(crate) const NAV_RADIUS: u8 = 6;
    pub(crate) const BADGE_RADIUS: u8 = 10;
    pub(crate) const MODAL_RADIUS: u8 = 8;

    pub(crate) const SECTION_GAP: f32 = 16.0;
    pub(crate) const CARD_PADDING: i8 = 18;
    pub(crate) const TITLE_TO_DESC: f32 = 6.0;
    pub(crate) const HEADER_TO_CONTENT: f32 = 18.0;

    pub(crate) const BUTTON_HEIGHT: f32 = 36.0;
    pub(crate) const TABLE_HEADER_HEIGHT: f32 = 34.0;
    pub(crate) const TABLE_ROW_HEIGHT: f32 = 34.0;
    /// 侧栏条目要高出一截:每个步骤条目是「标题 + 提示」两行文案。
    pub(crate) const NAV_ITEM_HEIGHT: f32 = 56.0;
    pub(crate) const SIDEBAR_WIDTH: f32 = 196.0;

    // ---------- 框架:卡片 / 分隔线 ----------

    /// 白色内容卡片:1px 边框 + 8px 圆角 + 极弱阴影。
    /// 内边距为 0,由调用方决定内部 padding。
    pub(crate) fn card_frame() -> egui::Frame {
        egui::Frame::new()
            .inner_margin(egui::Margin::ZERO)
            .fill(Self::surface())
            .stroke(Stroke::new(1.0, Self::line()))
            .corner_radius(CornerRadius::same(Self::CARD_RADIUS))
            .shadow(Shadow {
                offset: [0, 1],
                blur: 6,
                spread: 0,
                color: Color32::from_black_alpha(10),
            })
    }

    /// 卡片 + 标准 18px 内边距。
    pub(crate) fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
        Self::card_frame().show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::same(Self::CARD_PADDING))
                .show(ui, add_contents);
        });
    }

    /// 卡片内标题(16px strong ink)。
    pub(crate) fn card_title(ui: &mut egui::Ui, title: &str) {
        ui.label(
            egui::RichText::new(title)
                .size(16.0)
                .strong()
                .color(Self::ink()),
        );
    }

    /// 一条细分隔线(跨当前可用宽度)。
    pub(crate) fn thin_divider(ui: &mut egui::Ui) {
        ui.add_space(10.0);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().hline(
            rect.x_range(),
            rect.center().y,
            Stroke::new(1.0, Self::divider()),
        );
        ui.add_space(10.0);
    }

    // ---------- 按钮 ----------

    /// 自绘按钮:egui 的 `Button::fill` 会盖掉 hover 态,要 hover/禁用各自
    /// 一套颜色只能自己量尺寸、自己画。
    // 统一接收布局参数及普通、悬停、禁用三种样式,仅此绘制函数允许较多参数。
    #[allow(clippy::too_many_arguments)]
    fn button(
        ui: &mut egui::Ui,
        icon: Option<Icon>,
        text: &str,
        min_width: f32,
        height: f32,
        enabled: bool,
        idle: BtnLook,
        hover: BtnLook,
        disabled: BtnLook,
    ) -> egui::Response {
        let enabled = enabled && ui.is_enabled();
        let font = egui::FontId::proportional(15.0);
        let icon_slot = if icon.is_some() { 24.0 } else { 0.0 };
        let text_w = ui
            .painter()
            .layout_no_wrap(text.to_owned(), font.clone(), idle.text)
            .size()
            .x;
        let width = min_width.max(text_w + icon_slot + 24.0);
        let sense = if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), sense);
        let look = if !enabled {
            disabled
        } else if response.hovered() {
            hover
        } else {
            idle
        };
        if ui.is_rect_visible(rect) {
            ui.painter().rect(
                rect,
                CornerRadius::same(Self::BUTTON_RADIUS),
                look.fill,
                look.stroke,
                egui::StrokeKind::Inside,
            );
            let galley = ui
                .painter()
                .layout_no_wrap(text.to_owned(), font, look.text);
            let total = icon_slot + galley.size().x;
            let mut x = rect.center().x - total / 2.0;
            if let Some(icon) = icon {
                let icon_rect = egui::Rect::from_center_size(
                    egui::pos2(x + 9.0, rect.center().y),
                    egui::vec2(18.0, 18.0),
                );
                Self::paint_icon(ui.painter(), icon, icon_rect, look.text);
                x += icon_slot;
            }
            ui.painter().galley(
                egui::pos2(x, rect.center().y - galley.size().y / 2.0),
                galley,
                look.text,
            );
        }
        if enabled && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response
    }

    /// 主按钮:蓝底白字。
    pub(crate) fn primary_button(
        ui: &mut egui::Ui,
        icon: Option<Icon>,
        text: &str,
        min_width: f32,
        enabled: bool,
    ) -> egui::Response {
        Self::button(
            ui,
            icon,
            text,
            min_width,
            Self::BUTTON_HEIGHT,
            enabled,
            BtnLook {
                fill: Self::blue(),
                stroke: Stroke::NONE,
                text: Color32::WHITE,
            },
            BtnLook {
                fill: Self::accent_hover(),
                stroke: Stroke::NONE,
                text: Color32::WHITE,
            },
            BtnLook {
                fill: Self::disabled_bg(),
                stroke: Stroke::NONE,
                text: Self::text_disabled(),
            },
        )
    }

    /// 次按钮:白底灰边;hover 变蓝字蓝边。
    pub(crate) fn secondary_button(
        ui: &mut egui::Ui,
        icon: Option<Icon>,
        text: &str,
        min_width: f32,
        enabled: bool,
    ) -> egui::Response {
        Self::button(
            ui,
            icon,
            text,
            min_width,
            Self::BUTTON_HEIGHT,
            enabled,
            BtnLook {
                fill: Self::surface(),
                stroke: Stroke::new(1.0, Self::line_strong()),
                text: Self::ink(),
            },
            BtnLook {
                fill: Self::blue_soft(),
                stroke: Stroke::new(1.0, Self::accent_border()),
                text: Self::blue(),
            },
            BtnLook {
                fill: Self::surface_subtle(),
                stroke: Stroke::new(1.0, Self::line()),
                text: Self::text_disabled(),
            },
        )
    }

    // ---------- 徽标 / 进度条 / 提示条 ----------

    /// 胶囊状态徽标(软底色 + 同色文字 + 可选小图标)。
    pub(crate) fn status_badge(
        ui: &mut egui::Ui,
        icon: Option<Icon>,
        text: &str,
        fill: Color32,
        fg: Color32,
    ) {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 5))
            .fill(fill)
            .corner_radius(CornerRadius::same(Self::BADGE_RADIUS))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if let Some(icon) = icon {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        Self::paint_icon(ui.painter(), icon, rect, fg);
                    }
                    ui.label(egui::RichText::new(text).size(13.0).color(fg));
                });
            });
    }

    /// A/B 字母徽标:26×26 圆角色块 + 白字。
    pub(crate) fn letter_badge(ui: &mut egui::Ui, letter: &str, color: Color32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(6), color);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            letter,
            egui::FontId::proportional(15.0),
            Color32::WHITE,
        );
    }

    /// 进度条:浅灰轨道 + 蓝色填充,条内居中显示百分比(填充区白字、其余深色字)。
    pub(crate) fn progress_bar(ui: &mut egui::Ui, fraction: f32, width: f32) {
        let height = 14.0;
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(width.max(60.0), height), egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(7), Self::track());
        let fill_w = rect.width() * fraction.clamp(0.0, 1.0);
        if fill_w > 0.0 {
            let fill = egui::Rect::from_min_size(rect.min, egui::vec2(fill_w, rect.height()));
            painter.rect_filled(fill, CornerRadius::same(7), Self::blue());
        }
        let text = format!("{}%", (fraction.clamp(0.0, 1.0) * 100.0).round() as u32);
        let font = egui::FontId::proportional(11.0);
        let pos = rect.center()
            - painter
                .layout_no_wrap(text.clone(), font.clone(), Self::ink())
                .size()
                / 2.0;
        let filled =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.min.x + fill_w, rect.max.y));
        let unfilled = egui::Rect::from_min_max(filled.max, rect.max);
        painter.with_clip_rect(filled).text(
            pos,
            egui::Align2::LEFT_TOP,
            &text,
            font.clone(),
            Color32::WHITE,
        );
        painter.with_clip_rect(unfilled).text(
            pos,
            egui::Align2::LEFT_TOP,
            &text,
            font,
            Self::ink(),
        );
    }

    /// 浅黄底警告条(读取失败等)。
    pub(crate) fn warn_banner(ui: &mut egui::Ui, text: &str) {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 8))
            .fill(Self::warning_soft())
            .corner_radius(CornerRadius::same(6))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(15.0, 15.0), egui::Sense::hover());
                    Self::paint_icon(ui.painter(), Icon::Warn, rect, Self::amber());
                    ui.label(egui::RichText::new(text).size(13.0).color(Self::amber()));
                });
            });
    }

    // ---------- 工作流面板 / 底部操作行 ----------

    /// 步骤工作面板:卡片头(序号 + 标题 + 提示 + 右侧状态徽标)+ 细分隔线 + 内容区。
    pub(crate) fn work_panel(
        ui: &mut egui::Ui,
        index: &str,
        title: &str,
        hint: &str,
        status: Option<&str>,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) {
        Self::card_frame().show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: Self::CARD_PADDING,
                    right: Self::CARD_PADDING,
                    top: 14,
                    bottom: 0,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(index)
                                .size(13.0)
                                .strong()
                                .color(Self::blue()),
                        );
                        Self::card_title(ui, title);
                        ui.label(egui::RichText::new(hint).size(13.0).color(Self::muted()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(status) = status {
                                let busy = status.contains("正在")
                                    || status.contains('中')
                                    || status.contains('…');
                                if busy {
                                    Self::status_badge(
                                        ui,
                                        Some(Icon::Loader),
                                        status,
                                        Self::blue_soft(),
                                        Self::blue(),
                                    );
                                } else {
                                    Self::status_badge(
                                        ui,
                                        Some(Icon::CheckCircle),
                                        status,
                                        Self::teal_soft(),
                                        Self::teal(),
                                    );
                                }
                            }
                        });
                    });
                });
            Self::thin_divider(ui);
            egui::Frame::new()
                .inner_margin(egui::Margin::same(Self::CARD_PADDING))
                .show(ui, add_contents);
        });
    }

    /// 底部的说明 + 操作行。
    ///
    /// `tone` 决定左侧说明的配色:`Warn` 用于「当前配置还不能执行」这类状态文案。
    /// 状态提示要放在说明区:宽窗口下按钮闭包跑在 `right_to_left` 布局里,先添加的
    /// 控件排在最右,写在闭包里的提示会跑到主按钮右侧,读起来像按钮的附属说明。
    pub(crate) fn action_row(
        ui: &mut egui::Ui,
        note: &str,
        tone: NoteTone,
        add_buttons: impl FnOnce(&mut egui::Ui),
    ) {
        ui.add_space(17.0);
        ui.separator();
        ui.add_space(15.0);
        // 窗口较窄时让按钮逐个参与换行;宽窗口仍保持说明在左、操作在右的布局。
        let compact = ui.available_width() < 620.0;
        if compact {
            ui.horizontal_wrapped(|ui| {
                Self::action_note(ui, note, tone);
                ui.add_space(17.0);
                add_buttons(ui);
            });
        } else {
            ui.horizontal(|ui| {
                Self::action_note(ui, note, tone);
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    add_buttons,
                );
            });
        }
    }

    /// 说明行的小图标 + 文案:Warn 用三角警示,Normal 用对勾。
    pub(crate) fn action_note(ui: &mut egui::Ui, note: &str, tone: NoteTone) {
        let (icon, icon_color, text_color) = match tone {
            NoteTone::Normal => (Icon::CheckCircle, Self::teal(), Self::muted()),
            NoteTone::Warn => (Icon::Warn, Self::amber(), Self::amber()),
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(15.0, 15.0), egui::Sense::hover());
            Self::paint_icon(ui.painter(), icon, rect, icon_color);
            ui.label(egui::RichText::new(note).size(13.0).color(text_color));
        });
    }

    // ---------- 选择小控件 ----------

    /// 输出列选择 chip:选中 = 蓝底蓝字「• x」,未选 = 白底灰字「＋ x」。
    pub(crate) fn toggle_chip(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
        let (fill, stroke, text) = if selected {
            (
                Self::blue_soft(),
                Stroke::new(1.0, Self::accent_border()),
                egui::RichText::new(format!("• {label}"))
                    .size(13.0)
                    .color(Self::blue()),
            )
        } else {
            (
                Self::white(),
                Stroke::new(1.0, Self::line_strong()),
                egui::RichText::new(format!("＋ {label}"))
                    .size(13.0)
                    .color(Self::muted()),
            )
        };
        ui.add(
            egui::Button::new(text)
                .min_size(egui::vec2(0.0, 29.0))
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(Self::BUTTON_RADIUS)),
        )
    }

    /// 开关:34×18 胶囊轨道(on=主色 / off=轨道灰),白圆 knob 滑动切换。
    pub(crate) fn toggle_switch(ui: &mut egui::Ui, value: &mut bool, label: &str) {
        ui.horizontal(|ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(34.0, 18.0), egui::Sense::click());
            if response.clicked() {
                *value = !*value;
            }
            if ui.is_rect_visible(rect) {
                ui.painter().rect_filled(
                    rect,
                    rect.height() / 2.0,
                    if *value { Self::blue() } else { Self::track() },
                );
                let half = rect.height() / 2.0;
                let t = ui.ctx().animate_bool(response.id, *value);
                let knob_x = (rect.left() + half) * (1.0 - t) + (rect.right() - half) * t;
                ui.painter().circle(
                    egui::pos2(knob_x, rect.center().y),
                    half - 3.0,
                    Self::surface(),
                    Stroke::new(
                        1.0,
                        if response.hovered() {
                            Self::blue()
                        } else {
                            Self::line_strong()
                        },
                    ),
                );
            }
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            ui.label(egui::RichText::new(label).size(13.0).color(Self::muted()));
        });
    }

    // ---------- 图标绘制(painter 线条,不引图标库) ----------

    /// 在 rect 内画一个线条图标;rect 通常是 16~18px 见方。
    pub(crate) fn paint_icon(
        painter: &egui::Painter,
        icon: Icon,
        rect: egui::Rect,
        color: Color32,
    ) {
        let s = rect.width().min(rect.height());
        let c = rect.center();
        let l = c.x - s * 0.5;
        let r = c.x + s * 0.5;
        let t = c.y - s * 0.5;
        let b = c.y + s * 0.5;
        let w = (s * 0.085).clamp(1.1, 1.6);
        let st = Stroke::new(w, color);
        let line = |p: &egui::Painter, pts: Vec<egui::Pos2>| {
            p.add(egui::Shape::line(pts, st));
        };
        match icon {
            Icon::Doc => {
                let x0 = l + s * 0.20;
                let x1 = r - s * 0.20;
                let y0 = t + s * 0.12;
                let y1 = b - s * 0.12;
                let fx = x1 - s * 0.26;
                let fy = y0 + s * 0.26;
                line(
                    painter,
                    vec![
                        egui::pos2(x0, y0),
                        egui::pos2(fx, y0),
                        egui::pos2(x1, fy),
                        egui::pos2(x1, y1),
                        egui::pos2(x0, y1),
                        egui::pos2(x0, y0),
                    ],
                );
                line(
                    painter,
                    vec![egui::pos2(fx, y0), egui::pos2(fx, fy), egui::pos2(x1, fy)],
                );
            }
            Icon::DocPlus => {
                Self::paint_icon(painter, Icon::Doc, rect, color);
                let cx = r - s * 0.16;
                let cy = b - s * 0.18;
                painter.line_segment(
                    [egui::pos2(cx - s * 0.11, cy), egui::pos2(cx + s * 0.11, cy)],
                    Stroke::new(w, color),
                );
                painter.line_segment(
                    [egui::pos2(cx, cy - s * 0.11), egui::pos2(cx, cy + s * 0.11)],
                    Stroke::new(w, color),
                );
            }
            Icon::CheckCircle => {
                painter.circle_stroke(c, s * 0.36, st);
                line(
                    painter,
                    vec![
                        egui::pos2(c.x - s * 0.16, c.y + s * 0.01),
                        egui::pos2(c.x - s * 0.04, c.y + s * 0.14),
                        egui::pos2(c.x + s * 0.18, c.y - s * 0.14),
                    ],
                );
            }
            Icon::Warn => {
                line(
                    painter,
                    vec![
                        egui::pos2(c.x, t + s * 0.10),
                        egui::pos2(r - s * 0.12, b - s * 0.14),
                        egui::pos2(l + s * 0.12, b - s * 0.14),
                        egui::pos2(c.x, t + s * 0.10),
                    ],
                );
                painter.line_segment(
                    [
                        egui::pos2(c.x, t + s * 0.38),
                        egui::pos2(c.x, c.y + s * 0.10),
                    ],
                    st,
                );
                painter.circle_filled(egui::pos2(c.x, b - s * 0.26), w * 0.55, color);
            }
            Icon::Loader => {
                let rr = s * 0.32;
                let pts: Vec<egui::Pos2> = (0..=10)
                    .map(|i| {
                        let a = -1.6 + i as f32 * 4.4 / 10.0;
                        let (sin, cos) = a.sin_cos();
                        c + egui::vec2(cos * rr, sin * rr)
                    })
                    .collect();
                line(painter, pts);
            }
            Icon::External => {
                let x0 = l + s * 0.14;
                let y0 = t + s * 0.36;
                let x1 = r - s * 0.36;
                let y1 = b - s * 0.14;
                line(
                    painter,
                    vec![
                        egui::pos2(x0 + s * 0.34, y0),
                        egui::pos2(x0, y0),
                        egui::pos2(x0, y1),
                        egui::pos2(x1, y1),
                        egui::pos2(x1, y0 + s * 0.34),
                    ],
                );
                let tip = egui::pos2(r - s * 0.12, t + s * 0.12);
                painter.line_segment([egui::pos2(c.x, c.y), tip], st);
                painter.line_segment([egui::pos2(tip.x - s * 0.18, tip.y), tip], st);
                painter.line_segment([egui::pos2(tip.x, tip.y + s * 0.18), tip], st);
            }
        }
    }

    // ---------- 全局样式 ----------

    /// 配置 egui 视觉样式:浅灰工作区 + 白色卡片 + 蓝色主色。
    pub(crate) fn configure_ui_style(ctx: &egui::Context) {
        let mut visuals = egui::Visuals::light();
        visuals.panel_fill = Self::canvas();
        visuals.window_fill = Self::surface();
        visuals.faint_bg_color = Self::surface_subtle();
        // extreme_bg_color 在亮色主题下是进度条轨道等「凹陷」区域的底色;
        // 输入框底色由 text_edit_bg_color 单独指定为白。
        visuals.extreme_bg_color = Self::track();
        visuals.text_edit_bg_color = Some(Self::surface());
        visuals.hyperlink_color = Self::blue();
        visuals.warn_fg_color = Self::amber();
        visuals.error_fg_color = Self::danger();
        visuals.window_corner_radius = CornerRadius::same(Self::MODAL_RADIUS);
        visuals.menu_corner_radius = CornerRadius::same(Self::INPUT_RADIUS);
        visuals.window_shadow = Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(18),
        };
        visuals.window_stroke = Stroke::new(1.0, Self::line());
        visuals.widgets.noninteractive.bg_fill = Self::surface();
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Self::line());
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Self::ink());
        visuals.widgets.inactive.bg_fill = Self::surface();
        visuals.widgets.inactive.weak_bg_fill = Self::surface();
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Self::line_strong());
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Self::ink());
        visuals.widgets.hovered.bg_fill = Self::blue_soft();
        visuals.widgets.hovered.weak_bg_fill = Self::blue_soft();
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Self::accent_border());
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Self::blue());
        visuals.widgets.active.bg_fill = Self::blue_soft();
        visuals.widgets.active.weak_bg_fill = Self::blue_soft();
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, Self::blue());
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, Self::blue());
        visuals.widgets.open.bg_fill = Self::surface();
        visuals.widgets.open.weak_bg_fill = Self::surface();
        visuals.widgets.open.bg_stroke = Stroke::new(1.0, Self::accent_border());
        visuals.widgets.open.fg_stroke = Stroke::new(1.0, Self::ink());
        visuals.widgets.noninteractive.corner_radius = CornerRadius::same(Self::INPUT_RADIUS);
        visuals.widgets.inactive.corner_radius = CornerRadius::same(Self::INPUT_RADIUS);
        visuals.widgets.hovered.corner_radius = CornerRadius::same(Self::INPUT_RADIUS);
        visuals.widgets.active.corner_radius = CornerRadius::same(Self::INPUT_RADIUS);
        visuals.widgets.open.corner_radius = CornerRadius::same(Self::INPUT_RADIUS);
        visuals.selection.bg_fill = Self::blue();
        visuals.selection.stroke = Stroke::new(1.0, Self::blue());
        // Windows 的桌面文字通常更接近像素对齐效果，关闭 egui 的子像素分箱可减少
        // 小字号 Latin 字符的发虚；CJK 字符本身不会启用该模式。
        if cfg!(windows) {
            visuals.text_options.subpixel_binning = false;
        }
        ctx.set_visuals(visuals);

        ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 8.0);
            style.spacing.interact_size = egui::vec2(30.0, 32.0);
            style.spacing.icon_width = 18.0;
            style.spacing.icon_width_inner = 12.0;
            style.spacing.icon_spacing = 6.0;
            style.spacing.combo_width = 150.0;
            style.spacing.window_margin = egui::Margin::same(10);

            style
                .text_styles
                .insert(egui::TextStyle::Small, egui::FontId::proportional(13.0));
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
            style
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
            style
                .text_styles
                .insert(egui::TextStyle::Monospace, egui::FontId::monospace(14.0));
            style
                .text_styles
                .insert(egui::TextStyle::Heading, egui::FontId::proportional(28.0));
        });
    }
}
