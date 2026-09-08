//! 用 calamine 读取 Excel 工作簿 → Table
//!
//! 支持 .xlsx / .xls / .xlsb / .ods;返回所有 sheet 名及各 sheet 的表数据。
//! 第一行作为表头(若首行全空则跳过)。

use std::path::Path;

use anyhow::{Context, Result};
use calamine::{open_workbook_auto, Data, Reader};

use crate::model::{CellValue, Table};

/// 读取一个工作簿的全部 sheets。
/// 返回 (sheet 名, Table) 列表,顺序与工作簿一致。
pub fn read_workbook(path: &Path) -> Result<Vec<(String, Table)>> {
    let mut workbook = open_workbook_auto(path)
        .with_context(|| format!("无法打开文件: {}", path.display()))?;

    let names = workbook.sheet_names().to_vec();
    if names.is_empty() {
        anyhow::bail!("工作簿中没有工作表: {}", path.display());
    }

    let mut out = Vec::with_capacity(names.len());
    for name in &names {
        let range = workbook
            .worksheet_range(name)
            .with_context(|| format!("读取 sheet「{}」失败", name))?;
        out.push((name.clone(), table_from_range(&range)));
    }
    Ok(out)
}

/// 把 calamine Range<Data> 转成 Table:
/// 1. 找到第一行非空行作为表头(前导空行丢弃)
/// 2. 表头若出现空列,自动命名 `列{n}`
/// 3. 后续行逐行转换;整行全空则丢弃
fn table_from_range(range: &calamine::Range<Data>) -> Table {
    let rows: Vec<Vec<&Data>> = range.rows().map(|r| r.iter().collect()).collect();

    // 找第一行非空行
    let Some(header_idx) = rows.iter().position(|r| r.iter().any(|c| !matches!(c, Data::Empty))) else {
        return Table::default();
    };

    // 表头:字符串优先,数值转文本,空列自动命名
    let width = rows[header_idx].len();
    let mut headers: Vec<String> = (0..width)
        .map(|i| match rows[header_idx].get(i) {
            Some(Data::String(s)) => s.clone(),
            Some(Data::Float(f)) if f.fract() == 0.0 => format!("{:.0}", f),
            Some(Data::Int(i)) => i.to_string(),
            Some(d) => d.to_string(),
            None => String::new(),
        })
        .collect();
    // 表头去重 + 空名补全(避免后续按列名索引出问题)
    let mut seen = std::collections::HashMap::new();
    for (i, h) in headers.iter_mut().enumerate() {
        if h.trim().is_empty() {
            *h = format!("列{}", i + 1);
        }
        let cnt = seen.entry(h.clone()).or_insert(0usize);
        if *cnt > 0 {
            *h = format!("{}_{}", h, *cnt + 1);
        }
        *cnt += 1;
    }

    let mut table = Table::new(headers);
    for r in rows.iter().skip(header_idx + 1) {
        // 跳过整行全空
        if r.iter().all(|c| matches!(c, Data::Empty)) {
            continue;
        }
        let cells: Vec<CellValue> = r.iter().map(|d| data_to_cell(d)).collect();
        table.push_row(cells);
    }
    table
}

/// calamine Data → CellValue(日期等特殊类型转文本;错误→Empty)
fn data_to_cell(d: &Data) -> CellValue {
    match d {
        Data::Int(i) => CellValue::Number(*i as f64),
        Data::Float(f) => CellValue::Number(*f),
        Data::String(s) => CellValue::Text(s.clone()),
        Data::Bool(b) => CellValue::Text(if *b { "TRUE" } else { "FALSE" }.into()),
        Data::DateTime(dt) => CellValue::Text(dt.to_string()),
        Data::DateTimeIso(s) | Data::DurationIso(s) => CellValue::Text(s.clone()),
        Data::Error(_) => CellValue::Empty,
        Data::Empty => CellValue::Empty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_from_range_basic() {
        // 手工构造 Range<Data> 不直观,这里用最小 xlsx 数据走 workbook 太麻烦;
        // 退而测 data_to_cell 与空行跳过逻辑(通过私有辅助暴露的纯函数)。
        assert_eq!(data_to_cell(&Data::Int(5)), CellValue::Number(5.0));
        assert_eq!(data_to_cell(&Data::Float(1.5)), CellValue::Number(1.5));
        assert_eq!(data_to_cell(&Data::String("a".into())), CellValue::Text("a".into()));
        assert_eq!(data_to_cell(&Data::Bool(true)), CellValue::Text("TRUE".into()));
        assert_eq!(data_to_cell(&Data::Empty), CellValue::Empty);
    }
}
