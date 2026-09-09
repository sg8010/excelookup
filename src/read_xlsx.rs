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
        let mut range = workbook
            .worksheet_range(name)
            .with_context(|| format!("读取工作表「{}」失败", name))?;
        out.push((name.clone(), table_from_range(&mut range)));
    }
    Ok(out)
}

/// 把 calamine Range<Data> 转成 Table:
/// 1. 找到第一行非空行作为表头(前导空行丢弃)
/// 2. 表头若出现空列,自动命名 `列{n}`
/// 3. 后续行逐行转换;整行全空则丢弃
///
/// 优化:不建中间 Vec<Vec<&Data>> 引用矩阵;利用可变索引逐行 mem::take
/// 移动 String 等数据,避免 30 万行级别下每个文本格的 clone。
fn table_from_range(range: &mut calamine::Range<Data>) -> Table {
    let width = range.width();
    let height = range.height();
    if width == 0 || height == 0 {
        return Table::default();
    }

    // 1. 引用方式找表头行(前导空行丢弃)
    let mut header_idx: Option<usize> = None;
    for ri in 0..height {
        let row = &range[ri];
        if row.iter().any(|c| !matches!(c, Data::Empty)) {
            header_idx = Some(ri);
            break;
        }
    }
    let Some(header_idx) = header_idx else {
        return Table::default();
    };

    // 2. 表头(读引用;字符串 clone 仅发生在表头行,开销可忽略)
    let mut headers: Vec<String> = (0..width)
        .map(|i| match range[header_idx].get(i) {
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

    // 3. 数据行:预分配行容器(常见每行≈width),逐行 mem::take 移动值
    let mut table = Table::new(headers);
    let nrows = height - header_idx - 1;
    table.rows.reserve(nrows);
    for ri in (header_idx + 1)..height {
        let row: &mut [Data] = &mut range[ri];
        // 先判整行是否全空(引用检查,不移动)
        if row.iter().all(|c| matches!(c, Data::Empty)) {
            continue;
        }
        let mut cells: Vec<CellValue> = Vec::with_capacity(width);
        for d in row.iter_mut() {
            cells.push(data_take_cell(d));
        }
        table.rows.push(cells);
    }
    table
}

/// 移动版 data→cell:把可变 Data 中的 String 等 take 出来(留下 Data::Empty),
/// 避免大表下每格字符串 clone。
fn data_take_cell(d: &mut Data) -> CellValue {
    match d {
        Data::String(s) => CellValue::Text(std::mem::take(s)),
        Data::DateTimeIso(s) | Data::DurationIso(s) => CellValue::Text(std::mem::take(s)),
        Data::Int(i) => CellValue::Number(*i as f64),
        Data::Float(f) => CellValue::Number(*f),
        Data::Bool(b) => CellValue::Text(if *b { "TRUE" } else { "FALSE" }.into()),
        Data::DateTime(dt) => CellValue::Text(dt.to_string()),
        Data::Error(_) => CellValue::Empty,
        Data::Empty => CellValue::Empty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 nrow×ncol 的 Range,再用 set_value 填充指定 (相对行,列) 为值。
    fn make_range(
        nrows: usize,
        ncols: usize,
        cells: &[((usize, usize), Data)],
    ) -> calamine::Range<Data> {
        let mut r = calamine::Range::new(
            (0, 0),
            (
                (nrows as u32).saturating_sub(1),
                (ncols as u32).saturating_sub(1),
            ),
        );
        for ((row, col), d) in cells {
            r.set_value((*row as u32, *col as u32), d.clone());
        }
        r
    }

    fn s(v: &str) -> Data {
        Data::String(v.to_owned())
    }

    #[test]
    fn table_from_range_basic() {
        // 表头 id/name + 两数据行(含空行、数值、前导空行丢弃)
        let mut r = make_range(
            5,
            2,
            &[
                ((0, 0), Data::Empty), // 前导空行
                ((0, 1), Data::Empty),
                ((1, 0), s("id")),
                ((1, 1), s("name")),
                ((2, 0), s("1")),
                ((2, 1), s("alice")),
                ((3, 0), Data::Empty), // 全空行 → 跳过
                ((3, 1), Data::Empty),
                ((4, 0), Data::Float(2.0)),
                ((4, 1), s("bob")),
            ],
        );
        let t = table_from_range(&mut r);
        assert_eq!(t.headers, vec!["id", "name"]);
        assert_eq!(t.row_count(), 2);
        assert_eq!(t.cell(0, 0), Some(&CellValue::Text("1".into())));
        assert_eq!(t.cell(0, 1), Some(&CellValue::Text("alice".into())));
        assert_eq!(t.cell(1, 0), Some(&CellValue::Number(2.0)));
        assert_eq!(t.cell(1, 1), Some(&CellValue::Text("bob".into())));
    }

    #[test]
    fn table_from_range_dup_empty_header() {
        // 表头空列自动命名 + 重复名去重
        let mut r = make_range(
            2,
            3,
            &[
                ((0, 0), s("id")),
                ((0, 1), Data::Empty),
                ((0, 2), s("id")),
                ((1, 0), s("a")),
                ((1, 1), s("b")),
                ((1, 2), s("c")),
            ],
        );
        let t = table_from_range(&mut r);
        assert_eq!(t.headers, vec!["id", "列2", "id_2"]);
        assert_eq!(t.row_count(), 1);
    }

    #[test]
    fn table_from_range_all_empty() {
        let mut r = calamine::Range::<Data>::empty();
        let t = table_from_range(&mut r);
        assert!(t.is_empty());
    }

    #[test]
    fn data_take_cell_moves_string() {
        // 移动语义:内部 String 被移出(变空串),不再发生堆分配复制
        let mut d = Data::String("hello".to_owned());
        let v = data_take_cell(&mut d);
        assert_eq!(v, CellValue::Text("hello".into()));
        // 原值被掏空;Range 本身用完即弃,不影响业务
        assert!(matches!(d, Data::String(s) if s.is_empty()));

        let mut e = Data::DateTimeIso("2024-01-01".to_owned());
        assert_eq!(data_take_cell(&mut e), CellValue::Text("2024-01-01".into()));

        assert_eq!(data_take_cell(&mut Data::Int(5)), CellValue::Number(5.0));
        assert_eq!(data_take_cell(&mut Data::Float(1.5)), CellValue::Number(1.5));
        assert_eq!(data_take_cell(&mut Data::Bool(true)), CellValue::Text("TRUE".into()));
        assert_eq!(data_take_cell(&mut Data::Empty), CellValue::Empty);
    }
}
