//! 用 calamine 读取 Excel 工作簿 → Table
//!
//! 支持 .xlsx / .xls / .xlsb / .ods;返回所有 sheet 名及各 sheet 的表数据。
//! 列名行默认取首个非空行(前导空行跳过),也可由调用方指定——首行是合并大标题、
//! 第二行才是列名时,用 [`ReadOptions::header_row`] 指定第二行即可。
//!
//! **两条读表路径**
//!
//! - `Range`(`table_from_range`):calamine 先展开成稠密 `Range<Data>`,再转
//!   `Table`。所有格式通用,是等价性的参考答案。
//! - 流式(`stream` 子模块):xlsx/xlsm 逐格直接建 `Table`,不驻留稠密矩阵。
//!   大表单表峰值可降 20%~58%(见 `stream.rs` 模块文档)。
//!
//! 分派依据是 [`calamine::Sheets`] 的变体——它是 `open_workbook_auto` 按**内容**
//! 探测出来的结果,不是文件扩展名(无扩展名/未知扩展名的 xlsx 也能被正确识别)。
//! 用同一份判断做分派,不会出现"扩展名说是 xlsx、内容其实是 xls"的错配。
//!
//! 流式失败/不适用时回退 `Range` 路径。回退会**重新打开工作簿**而不是复用当前
//! handle:`XlsxCellReader` 会部分推进那个 zip 成员流,复用依赖 calamine 的内部
//! 实现细节;重开一次只花约 0.5ms,相对读表耗时可忽略。

pub mod stream;

use std::path::Path;

use anyhow::{Context, Result};
use calamine::{open_workbook_auto, Data, Reader, Sheets};

use crate::model::{CellValue, Table};
use stream::StreamRead;

/// 顶部原始行预览默认条数(UI 用它列出可选列名行)
pub const PREVIEW_ROWS: usize = 8;

/// 读取选项
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadOptions {
    /// 各工作表的列名行:按工作表出现顺序,元素为已用区域内 0-based 行号。
    /// 缺项(下表越界)或元素为 `None` = 自动(取该表首个非空行)。
    /// 指定行越界、或整行全空时也会回退为自动——避免工作表行数不同时把整表读废。
    pub header_rows: Vec<Option<usize>>,
    /// 是否回传顶部原始行预览(供 UI 展示/选择列名行)
    pub preview: bool,
}

/// 一个工作表:数据表 + 换列名行所需的上下文
#[derive(Debug, Clone, Default)]
pub struct SheetTable {
    /// 工作表名
    pub name: String,
    /// 按 [`ReadOptions`] 选定列名行后构造的表
    pub table: Table,
    /// 已用区域顶部前 [`PREVIEW_ROWS`] 行(按展示文本;含被当作列名的行)
    pub preview: Vec<Vec<String>>,
    /// 预览行的类型化单元格,供 GUI 在预览窗口内切换列名行时复用数据。
    pub preview_cells: Vec<Vec<CellValue>>,
    /// 预览行是否包含至少一个原始非空单元格(Data::String("")也算非空)
    /// (错误单元格也算非空,与原始 Range 的判断保持一致)。
    pub preview_non_empty: Vec<bool>,
    /// 自动检测到的列名行(已用区域 0-based);整表全空为 None
    pub auto_header_row: Option<usize>,
    /// 实际用作列名的行(已用区域 0-based):指定行越界/全空而回退自动时与
    /// [`ReadOptions::header_row`] 不同,UI 据此提示用户
    pub used_header_row: Option<usize>,
    /// 已用区域首行对应的 Excel 行号(1-based),UI 用它显示"第 N 行"
    pub first_row_number: usize,
}

/// 读取一个工作簿的全部 sheets(列名行自动检测)。
/// 返回 (sheet 名, Table) 列表,顺序与工作簿一致。
pub fn read_workbook(path: &Path) -> Result<Vec<(String, Table)>> {
    Ok(read_workbook_opts(path, ReadOptions::default())?
        .into_iter()
        .map(|s| (s.name, s.table))
        .collect())
}

/// 读取一个工作簿的全部 sheets,带读取选项(列名行可按工作表指定)。
///
/// 与 [`read_sheet_opts`] 一样按内容分派路径;逐表复用同一个已打开的工作簿
/// (open 要解析 sharedStrings,大表上不能免费重做)。单表流式不适用/需回退时,
/// 该表退回 `Range` 路径 —— **不再重开工作簿**,因为 `worksheet_range` 会自己新建
/// 一个单元格读取器,不受当前表流式读取的影响。
pub fn read_workbook_opts(path: &Path, opts: ReadOptions) -> Result<Vec<SheetTable>> {
    let mut workbook = open_workbook_auto(path)
        .with_context(|| format!("无法打开文件: {}", path.display()))?;

    let names = workbook.sheet_names().to_vec();
    if names.is_empty() {
        anyhow::bail!("工作簿中没有工作表: {}", path.display());
    }

    let mut out = Vec::with_capacity(names.len());
    for (index, name) in names.iter().enumerate() {
        // 该表指定的列名行(未指定 = 自动)
        let requested = opts.header_rows.get(index).copied().flatten();
        let mut sheet =
            match stream::read_sheet_with_workbook(&mut workbook, name, requested, opts.preview)? {
            StreamRead::Done(sheet) => sheet,
            // chartsheet 等非工作表:与 Range 路径一致地返回空表
            StreamRead::NotAWorksheet => SheetTable::default(),
            // 非 xlsx 或结构畸形:本表退回 Range 路径(同一 handle 上新建读取器)
            StreamRead::Unsupported | StreamRead::Fallback => {
                let mut range = workbook
                    .worksheet_range(name)
                    .with_context(|| format!("读取工作表「{}」失败", name))?;
                table_from_range(&mut range, requested, opts.preview)
            }
        };
        sheet.name = name.clone();
        out.push(sheet);
    }
    Ok(out)
}

/// 读表路径(测试用开关;生产环境由 [`read_sheet_opts`] 自动分派)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadPath {
    /// 自动:xlsx 走流式,其余走 `Range`;流式不适用时回退
    Auto,
    /// 强制 `Range` 路径(稠密矩阵)
    Range,
    /// 强制流式(非 xlsx 或结构畸形时报错)
    Stream,
}

/// 只读取工作簿中的一个工作表。
///
/// 改列名行时调用此函数即可,避免为了重建当前表而重新解析整个工作簿。
/// 走哪条路径见 [`ReadPath`]。
pub fn read_sheet_opts(
    path: &Path,
    sheet_name: &str,
    requested: Option<usize>,
    preview: bool,
) -> Result<SheetTable> {
    read_sheet_opts_path(path, sheet_name, requested, preview, ReadPath::Auto)
}

/// 带路径开关的读表入口,供差分测试同时驱动两条实现。
pub fn read_sheet_opts_path(
    path: &Path,
    sheet_name: &str,
    requested: Option<usize>,
    preview: bool,
    read_path: ReadPath,
) -> Result<SheetTable> {
    if read_path != ReadPath::Range {
        match stream::read_sheet_stream(path, sheet_name, requested, preview)? {
            StreamRead::Done(sheet) => {
                return Ok(SheetTable {
                    name: sheet_name.to_owned(),
                    ..sheet
                });
            }
            // chartsheet 等非工作表:Range 路径返回 Ok(空表),这里保持一致。
            // 不能退化成报错,否则含图表页的工作簿会从"显示空表"变成"打开失败"。
            StreamRead::NotAWorksheet => {
                return Ok(SheetTable {
                    name: sheet_name.to_owned(),
                    ..SheetTable::default()
                });
            }
            StreamRead::Unsupported | StreamRead::Fallback => {
                if read_path == ReadPath::Stream {
                    anyhow::bail!("该文件无法流式读取,请改用 Range 路径");
                }
                // 回退:重开工作簿(不复用已推进的 handle)
            }
        }
    }
    read_sheet_range(path, sheet_name, requested, preview)
}

/// `Range` 路径:稠密矩阵 → `Table`(两条路径的等价性参考答案)
pub(crate) fn read_sheet_range(
    path: &Path,
    sheet_name: &str,
    requested: Option<usize>,
    preview: bool,
) -> Result<SheetTable> {
    let mut workbook = open_workbook_auto(path)
        .with_context(|| format!("无法打开文件: {}", path.display()))?;
    let mut range = workbook
        .worksheet_range(sheet_name)
        .with_context(|| format!("读取工作表「{}」失败", sheet_name))?;
    let mut sheet = table_from_range(&mut range, requested, preview);
    sheet.name = sheet_name.to_owned();
    Ok(sheet)
}

/// 该文件是否会被走流式路径(内容探测,不看扩展名)。仅用于诊断日志。
pub fn supports_stream_read(path: &Path) -> bool {
    matches!(open_workbook_auto(path), Ok(Sheets::Xlsx(_)))
}

/// 已用区域内第一个非空行(前导空行全部跳过)
fn first_non_empty_row(range: &calamine::Range<Data>, height: usize) -> Option<usize> {
    (0..height).find(|&ri| range[ri].iter().any(|c| !matches!(c, Data::Empty)))
}

/// 预览用文本(与 [`CellValue::display`] 口径一致:整数不带小数点)
pub(crate) fn data_display(d: &Data) -> String {
    match d {
        Data::Float(f) if f.fract() == 0.0 && f.is_finite() => format!("{:.0}", f),
        Data::Empty => String::new(),
        other => other.to_string(),
    }
}

/// 把 calamine Range<Data> 转成 [`SheetTable`]:
/// 1. 定位列名行:`requested` 有效则用它,否则取首个非空行(前导空行丢弃)
/// 2. 列名若出现空列,自动命名 `列{n}`;重名加序号去重
/// 3. 列名行之后逐行转换;整行全空则丢弃
///
/// 优化:不建中间 Vec<Vec<&Data>> 引用矩阵;利用可变索引逐行 mem::take
/// 移动 String 等数据,避免 30 万行级别下每个文本格的 clone。
fn table_from_range(
    range: &mut calamine::Range<Data>,
    requested: Option<usize>,
    preview_wanted: bool,
) -> SheetTable {
    let width = range.width();
    let height = range.height();
    if width == 0 || height == 0 {
        return SheetTable::default();
    }

    // 0. 顶部原始行预览(仅前 PREVIEW_ROWS 行,数据移动前先取文本)
    let (preview, preview_cells, preview_non_empty) = if preview_wanted {
        let mut preview = Vec::with_capacity(height.min(PREVIEW_ROWS));
        let mut preview_cells = Vec::with_capacity(height.min(PREVIEW_ROWS));
        let mut preview_non_empty = Vec::with_capacity(height.min(PREVIEW_ROWS));
        for ri in 0..height.min(PREVIEW_ROWS) {
            let row = &range[ri];
            preview_non_empty.push(row.iter().any(|c| !matches!(c, Data::Empty)));
            preview.push(row.iter().map(data_display).collect());
            preview_cells.push(row.iter().map(data_clone_cell).collect());
        }
        (preview, preview_cells, preview_non_empty)
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };

    // 1. 定位列名行:指定行越界或整行全空 → 回退自动
    let auto = first_non_empty_row(range, height);
    let header_idx = match requested {
        Some(k) if k < height && range[k].iter().any(|c| !matches!(c, Data::Empty)) => Some(k),
        _ => auto,
    };
    let Some(header_idx) = header_idx else {
        return SheetTable {
            preview,
            preview_cells,
            preview_non_empty,
            first_row_number: range.start().map_or(1, |(r, _)| r as usize + 1),
            ..SheetTable::default()
        };
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
    SheetTable {
        name: String::new(),
        table,
        preview,
        preview_cells,
        preview_non_empty,
        auto_header_row: auto,
        used_header_row: Some(header_idx),
        first_row_number: range.start().map_or(1, |(r, _)| r as usize + 1),
    }
}

/// 复制预览用的类型化单元格;预览最多只有 [`PREVIEW_ROWS`] 行,额外开销很小。
fn data_clone_cell(d: &Data) -> CellValue {
    let mut copied = d.clone();
    data_take_cell(&mut copied)
}

/// 移动版 data→cell:把可变 Data 中的 String 等 take 出来(留下 Data::Empty),
/// 避免大表下每格字符串 clone。
pub(crate) fn data_take_cell(d: &mut Data) -> CellValue {
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

    type Cells = Vec<((usize, usize), Data)>;

    /// 构造 nrows×ncol 的 Range,再用 set_value 填充指定 (相对行,列) 为值。
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

    /// 自动列名行、不要预览。
    /// 注意:读表对 Range 是移动语义(mem::take),每次调用都重新构造 Range。
    fn auto(nrows: usize, ncols: usize, cells: &[((usize, usize), Data)]) -> SheetTable {
        table_from_range(&mut make_range(nrows, ncols, cells), None, false)
    }

    /// 指定列名行(已用区域 0-based),带回顶部预览
    fn with_header(
        nrows: usize,
        ncols: usize,
        cells: &[((usize, usize), Data)],
        row: usize,
    ) -> SheetTable {
        table_from_range(&mut make_range(nrows, ncols, cells), Some(row), true)
    }

    #[test]
    fn table_from_range_basic() {
        // 表头 id/name + 两数据行(含空行、数值、前导空行丢弃)
        let cells: Cells = vec![
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
        ];
        let t = auto(5, 2, &cells);
        assert_eq!(t.table.headers, vec!["id", "name"]);
        assert_eq!(t.table.row_count(), 2);
        assert_eq!(t.table.cell(0, 0), Some(&CellValue::Text("1".into())));
        assert_eq!(t.table.cell(0, 1), Some(&CellValue::Text("alice".into())));
        assert_eq!(t.table.cell(1, 0), Some(&CellValue::Number(2.0)));
        assert_eq!(t.table.cell(1, 1), Some(&CellValue::Text("bob".into())));
        // 自动检测出的列名行 = 已用区域第 2 行(0-based 1);Excel 行号从 1 起
        assert_eq!(t.auto_header_row, Some(1));
        assert_eq!(t.first_row_number, 1);
    }

    #[test]
    fn table_from_range_dup_empty_header() {
        // 表头空列自动命名 + 重复名去重
        let cells: Cells = vec![
            ((0, 0), s("id")),
            ((0, 1), Data::Empty),
            ((0, 2), s("id")),
            ((1, 0), s("a")),
            ((1, 1), s("b")),
            ((1, 2), s("c")),
        ];
        let t = auto(2, 3, &cells);
        assert_eq!(t.table.headers, vec!["id", "列2", "id_2"]);
        assert_eq!(t.table.row_count(), 1);
    }

    #[test]
    fn table_from_range_all_empty() {
        let mut r = calamine::Range::<Data>::empty();
        let t = table_from_range(&mut r, None, false);
        assert!(t.table.is_empty());
        assert_eq!(t.auto_header_row, None);
    }

    /// 合并大标题行:第 1 行整行合并标题(calamine 只给锚点格值),第 2 行才是列名
    fn merged_title_cells() -> Cells {
        vec![
            ((0, 0), s("2024 年销售统计")),
            ((1, 0), s("id")),
            ((1, 1), s("名称")),
            ((1, 2), s("金额")),
            ((2, 0), Data::Float(1001.0)),
            ((2, 1), s("苹果")),
            ((2, 2), Data::Float(12.5)),
            ((3, 0), Data::Float(1002.0)),
            ((3, 1), s("香蕉")),
            ((3, 2), Data::Float(7.0)),
        ]
    }

    #[test]
    fn auto_header_takes_title_row() {
        // 自动 = 取首个非空行,于是大标题被当成列名(用户遇到的问题)
        let t = auto(4, 3, &merged_title_cells());
        assert_eq!(t.auto_header_row, Some(0));
        assert_eq!(t.table.headers[0], "2024 年销售统计");
        assert_eq!(t.table.headers[1], "列2"); // 合并区非锚点格为空 → 自动补名
        assert_eq!(t.table.row_count(), 3);
    }

    #[test]
    fn header_row_skips_merged_title_row() {
        // 指定第 2 行作列名 → 标题行被忽略,列名/数据都正确
        let t = with_header(4, 3, &merged_title_cells(), 1);
        assert_eq!(t.table.headers, vec!["id", "名称", "金额"]);
        assert_eq!(t.table.row_count(), 2);
        assert_eq!(t.table.cell(0, 0), Some(&CellValue::Number(1001.0)));
        assert_eq!(t.table.cell(1, 1), Some(&CellValue::Text("香蕉".into())));
        // 预览保留顶部原始内容(含被忽略的标题行);整数不带小数点
        assert_eq!(t.preview[0][0], "2024 年销售统计");
        assert_eq!(t.preview[1], vec!["id", "名称", "金额"]);
        assert_eq!(t.preview[2][0], "1001");
    }

    #[test]
    fn header_row_out_of_range_falls_back_to_auto() {
        let cells: Cells = vec![
            ((0, 0), s("id")),
            ((0, 1), s("v")),
            ((1, 0), s("1")),
            ((1, 1), s("x")),
        ];
        let t = with_header(2, 2, &cells, 9);
        assert_eq!(t.table.headers, vec!["id", "v"]);
        assert_eq!(t.table.row_count(), 1);
    }

    #[test]
    fn header_row_blank_row_falls_back_to_auto() {
        let cells: Cells = vec![
            ((0, 0), s("id")),
            ((0, 1), s("v")),
            // 第 2 行整行空
            ((2, 0), s("1")),
            ((2, 1), s("x")),
        ];
        let t = with_header(3, 2, &cells, 1);
        assert_eq!(t.table.headers, vec!["id", "v"]);
        assert_eq!(t.table.row_count(), 1);
    }

    #[test]
    fn header_row_two_level_header() {
        // 两行表头(首行分组名)时取第二行;自动则取首行并去重
        let cells: Cells = vec![
            ((0, 0), s("2024")),
            ((0, 1), s("2024")),
            ((1, 0), s("id")),
            ((1, 1), s("name")),
            ((2, 0), s("1")),
            ((2, 1), s("a")),
            ((3, 0), s("2")),
            ((3, 1), s("b")),
        ];
        assert_eq!(auto(4, 2, &cells).table.headers, vec!["2024", "2024_2"]);
        assert_eq!(with_header(4, 2, &cells, 1).table.headers, vec!["id", "name"]);
    }

    #[test]
    fn preview_capped_at_preview_rows() {
        let cells: Cells = (0..PREVIEW_ROWS + 4)
            .map(|i| ((i, 0), s(&format!("r{i}"))))
            .collect();
        let t = with_header(PREVIEW_ROWS + 4, 1, &cells, 1);
        assert_eq!(t.preview.len(), PREVIEW_ROWS);
        assert_eq!(t.preview[0][0], "r0");
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
