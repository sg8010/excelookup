//! 用 rust_xlsxwriter 把 Table 导出为 .xlsx(带简单样式:表头加粗、冻结首行)

use anyhow::{Context, Result};
use std::path::Path;

use rust_xlsxwriter::{Format, Workbook};

use crate::model::{CellValue, Table};

/// 导出 Table 到 xlsx 文件(单 sheet,名为 result)
pub fn write_xlsx(table: &Table, path: &Path) -> Result<()> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();

    // 表头样式:加粗 + 背景
    let header_fmt = Format::new()
        .set_bold()
        .set_background_color("DDEBF7")
        .set_border(rust_xlsxwriter::FormatBorder::Thin);
    // 自适应列宽按内容估算(粗略:取最大显示长度,上限 40)
    let mut widths: Vec<usize> = table
        .headers
        .iter()
        .map(|h| h.chars().count())
        .collect();

    // 写表头
    for (c, h) in table.headers.iter().enumerate() {
        sheet
            .write_string_with_format(0, c as u16, h, &header_fmt)
            .with_context(|| format!("写表头 {h} 失败"))?;
    }

    // 写数据
    for (r, row) in table.rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let col = c as u16;
            let row_i = (r + 1) as u32;
            match cell {
                CellValue::Number(n) => {
                    sheet.write_number(row_i, col, *n)?;
                }
                CellValue::Text(s) => {
                    sheet.write_string(row_i, col, s)?;
                }
                CellValue::Empty => {}
            }
            // 更新列宽(中文按 2 计)
            let w = cell.display().chars().map(|ch| if ch.is_ascii() { 1 } else { 2 }).sum();
            widths[c] = widths[c].max(w);
        }
    }

    // 冻结首行 + 设置列宽
    sheet.set_freeze_panes(1, 0)?;
    for (c, w) in widths.iter().enumerate() {
        let width = (*w as f64).min(40.0).max(8.0);
        sheet.set_column_width(c as u16, width)?;
    }

    workbook
        .save(path)
        .with_context(|| format!("保存文件失败: {}", path.display()))?;
    Ok(())
}
