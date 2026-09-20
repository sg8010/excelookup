//! 用 rust_xlsxwriter 把 Table 或 Join 结果视图导出为 .xlsx
//! (带简单样式:表头加粗、冻结首行)

use anyhow::{Context, Result};
use std::path::Path;

use rust_xlsxwriter::{Format, Workbook};

use crate::join::JoinedTable;
use crate::model::{CellValue, Table};

/// 导出阶段。写入阶段可以按行报告进度,保存阶段由 xlsx 打包器统一完成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportPhase {
    Writing,
    Saving,
}

/// 导出进度,供 GUI 或 CLI 订阅;不依赖任何 GUI 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportProgress {
    pub phase: ExportPhase,
    pub completed_rows: usize,
    pub total_rows: usize,
}

const COLUMN_WIDTH_SAMPLE_ROWS: usize = 4096;
const MAX_COLUMN_WIDTH: usize = 40;
const PROGRESS_INTERVAL: usize = 1024;

/// 导出 Table 到 xlsx 文件(单 sheet,名为 result)
pub fn write_xlsx(table: &Table, path: &Path) -> Result<()> {
    write_xlsx_with_progress(table, path, |_| {})
}

/// 导出 Table 到 xlsx 文件,并按阶段报告进度。
///
/// 数据按行递增写入,使用 `rust_xlsxwriter` 的 constant memory 模式,避免
/// 在大结果集上再维护完整的工作表单元格结构。回调只在开始、每隔一段行数
/// 以及进入保存阶段时调用,不会为每个单元格发送事件。
pub fn write_xlsx_with_progress(
    table: &Table,
    path: &Path,
    mut on_progress: impl FnMut(ExportProgress),
) -> Result<()> {
    write_export_with_progress(table, path, &mut on_progress)
}

/// 导出基于源表行引用的 Join 结果,不先物化完整结果表。
pub fn write_joined_xlsx(
    table: &JoinedTable,
    left: &Table,
    right: &Table,
    path: &Path,
) -> Result<()> {
    write_joined_xlsx_with_progress(table, left, right, path, |_| {})
}

/// 导出基于源表行引用的 Join 结果,并按阶段报告进度。
pub fn write_joined_xlsx_with_progress(
    table: &JoinedTable,
    left: &Table,
    right: &Table,
    path: &Path,
    mut on_progress: impl FnMut(ExportProgress),
) -> Result<()> {
    let source = JoinedExport { table, left, right };
    write_export_with_progress(&source, path, &mut on_progress)
}

trait ExportSource {
    fn headers(&self) -> &[String];
    fn row_count(&self) -> usize;
    fn cell(&self, row: usize, column: usize) -> Option<&CellValue>;
}

impl ExportSource for Table {
    fn headers(&self) -> &[String] {
        &self.headers
    }

    fn row_count(&self) -> usize {
        self.row_count()
    }

    fn cell(&self, row: usize, column: usize) -> Option<&CellValue> {
        self.cell(row, column)
    }
}

struct JoinedExport<'a> {
    table: &'a JoinedTable,
    left: &'a Table,
    right: &'a Table,
}

impl ExportSource for JoinedExport<'_> {
    fn headers(&self) -> &[String] {
        &self.table.headers
    }

    fn row_count(&self) -> usize {
        self.table.row_count()
    }

    fn cell(&self, row: usize, column: usize) -> Option<&CellValue> {
        self.table.cell(self.left, self.right, row, column)
    }
}

fn write_export_with_progress(
    source: &impl ExportSource,
    path: &Path,
    on_progress: &mut impl FnMut(ExportProgress),
) -> Result<()> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet_with_constant_memory();

    // 表头样式:加粗 + 背景
    let header_fmt = Format::new()
        .set_bold()
        .set_background_color("DDEBF7")
        .set_border(rust_xlsxwriter::FormatBorder::Thin);
    // 自适应列宽只采样前若干行,不再对大表的每个单元格生成 display String。
    let widths = estimate_column_widths(source);
    let total_rows = source.row_count();

    on_progress(ExportProgress {
        phase: ExportPhase::Writing,
        completed_rows: 0,
        total_rows,
    });

    // 写表头
    for (c, h) in source.headers().iter().enumerate() {
        sheet
            .write_string_with_format(0, c as u16, h, &header_fmt)
            .with_context(|| format!("写表头 {h} 失败"))?;
    }

    // 写数据
    for r in 0..total_rows {
        let row_i = u32::try_from(r + 1).context("导出行号超出 Excel 限制")?;
        for c in 0..widths.len() {
            let col = c as u16;
            match source.cell(r, c) {
                Some(CellValue::Number(n)) => {
                    sheet.write_number(row_i, col, *n)?;
                }
                Some(CellValue::Text(s)) => {
                    sheet.write_string(row_i, col, s)?;
                }
                Some(CellValue::Empty) | None => {}
            }
        }
        let completed_rows = r + 1;
        if completed_rows % PROGRESS_INTERVAL == 0 || completed_rows == total_rows {
            on_progress(ExportProgress {
                phase: ExportPhase::Writing,
                completed_rows,
                total_rows,
            });
        }
    }

    // 冻结首行 + 设置列宽
    sheet.set_freeze_panes(1, 0)?;
    for (c, w) in widths.iter().enumerate() {
        let width = (*w as f64).min(40.0).max(8.0);
        sheet.set_column_width(c as u16, width)?;
    }

    // 打包、压缩阶段无法按行细分,明确通知调用方避免界面看起来像卡死。
    on_progress(ExportProgress {
        phase: ExportPhase::Saving,
        completed_rows: total_rows,
        total_rows,
    });
    workbook
        .save(path)
        .with_context(|| format!("保存文件失败: {}", path.display()))?;
    Ok(())
}

/// 估算一列在 Excel 中的显示宽度。只检查前 4096 行,并把结果限制在 40。
/// 文本直接借用原字符串计算,数字只在采样行中格式化,因此不会为全表创建
/// 临时展示字符串。
fn estimate_column_widths(source: &impl ExportSource) -> Vec<usize> {
    let mut widths: Vec<usize> = source
        .headers()
        .iter()
        .map(|header| display_width(header).min(MAX_COLUMN_WIDTH))
        .collect();

    for row in 0..source.row_count().min(COLUMN_WIDTH_SAMPLE_ROWS) {
        for column in 0..widths.len() {
            if let Some(cell) = source.cell(row, column) {
                widths[column] = widths[column].max(cell_display_width(cell).min(MAX_COLUMN_WIDTH));
            }
        }
        if widths.iter().all(|width| *width >= MAX_COLUMN_WIDTH) {
            break;
        }
    }

    widths
}

/// 粗略模拟 Excel 宽度:ASCII 按 1 计,其他 Unicode 字符按 2 计。
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| if ch.is_ascii() { 1 } else { 2 })
        .sum()
}

fn cell_display_width(cell: &CellValue) -> usize {
    match cell {
        CellValue::Number(number) => {
            let text = if number.fract() == 0.0 && number.is_finite() {
                format!("{number:.0}")
            } else {
                number.to_string()
            };
            display_width(&text)
        }
        CellValue::Text(text) => display_width(text),
        CellValue::Empty => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn display_width_counts_non_ascii_as_two() {
        assert_eq!(display_width("A中"), 3);
    }

    #[test]
    fn width_estimate_is_sampled_and_capped() {
        let mut table = Table::new(vec!["标题".into(), "值".into()]);
        table.push_row(vec!["短".into(), "x".into()]);
        table.push_row(vec!["这是一个很长的文本".into(), "y".into()]);

        let widths = estimate_column_widths(&table);
        assert_eq!(widths[0], 18);
        assert_eq!(widths[1], 2);
    }

    #[test]
    fn export_reports_writing_and_saving() {
        let mut table = Table::new(vec!["编号".into(), "名称".into()]);
        table.push_row(vec![1.0.into(), "苹果".into()]);
        table.push_row(vec![2.0.into(), "香蕉".into()]);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应晚于 Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "excelookup-export-test-{}-{stamp}.xlsx",
            std::process::id()
        ));
        let mut events = Vec::new();
        write_xlsx_with_progress(&table, &path, |progress| events.push(progress)).unwrap();

        assert_eq!(
            events.first().map(|event| event.phase),
            Some(ExportPhase::Writing)
        );
        assert_eq!(
            events
                .iter()
                .find(|event| event.phase == ExportPhase::Writing && event.completed_rows == 2)
                .map(|event| event.total_rows),
            Some(2)
        );
        assert_eq!(
            events.last().map(|event| event.phase),
            Some(ExportPhase::Saving)
        );
        assert!(path.is_file());
        std::fs::remove_file(path).unwrap();
    }
}
