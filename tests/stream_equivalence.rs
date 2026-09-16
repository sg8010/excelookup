//! 流式读表 vs `Range` 读表的**差分测试**(逐格对拍)
//!
//! # 这是什么
//!
//! `read_xlsx` 有两条读表实现:
//!
//! - `ReadPath::Range` —— 先展开稠密 `Range<Data>`,再转 `Table`(旧路径,通用)
//! - `ReadPath::Stream` —— 由 `XlsxCellReader` 逐格直接建 `Table`(xlsx 专用)
//!
//! 两条路径**必须给出完全相同的结果**。本文件对同一个文件同时跑两条路径,然后把
//! 结果**逐字段、逐单元格**断言相等(而不是手写几个"应该是 42"的期望值):
//!
//! - `headers` / `table.rows` —— `CellValue` 是精确 `PartialEq`,整行 `Vec` 相等
//!   即"这一行每一格的类型与值都相同"
//! - `preview` / `preview_cells` / `preview_non_empty` —— 列名行选择器所依赖的顶部预览
//! - `auto_header_row` / `used_header_row` / `first_row_number` —— 列名行定位与回退
//!
//! 为什么要差分而不是手写断言:这条改动改的是"怎么把单元格搬进 `Table`",失效方式
//! 全是边角(前导空行、中间跳列、合并标题、错误值、稀疏列错位)。手写断言只覆盖想得
//! 到的情况;差分拿"已经在用的实现"当参考答案,能抓出**我没在找的东西**——本测试
//! 落地过程中它就已经抓出了一处 `Bool` 的大小写口径差异。
//!
//! 若哪天 `table_from_range` 被删除(两条路径合并成一条),本测试自动失效,那也正是
//! 它的退休时机。

use std::path::{Path, PathBuf};

use excelookup_lib::model::CellValue;
use excelookup_lib::read_xlsx::{
    ReadOptions, ReadPath, SheetTable, read_sheet_opts_path, read_workbook, read_workbook_opts,
};
use rust_xlsxwriter::Workbook;

/// 边界用例文件:覆盖读表层所有已知分支
fn make_edge_workbook(path: &Path) {
    let mut wb = Workbook::new();
    {
        // 前导空行 + 跳列(空列名) + 重名 + 数值/文本/布尔/错误 + 尾部稀疏列
        let s = wb.add_worksheet();
        s.set_name("basic").unwrap();
        // 第 0 行整行空(不写任何东西)→ 前导空行
        s.write_string(1, 0, "id").unwrap();
        s.write_string(1, 2, "id").unwrap(); // 跳过 B 列 → 空列名 + 重名
        s.write_string(1, 3, "值").unwrap();
        s.write_string(2, 0, "1").unwrap();
        s.write_number(2, 2, 2.0).unwrap();
        s.write_number(2, 3, 12.5).unwrap();
        s.write_boolean(3, 0, true).unwrap();
        s.write_string(3, 3, "文本").unwrap();
        s.write_formula(4, 0, "=1/0").unwrap(); // 错误值
        s.write_string(4, 3, "尾").unwrap();
        s.write_number(6, 5, 9.0).unwrap(); // 尾部更靠右的稀疏列
    }
    {
        // 合并大标题 + 真正的列名在第二行
        let s = wb.add_worksheet();
        s.set_name("titled").unwrap();
        s.write_string(0, 0, "2024 年销售").unwrap();
        s.write_string(1, 0, "id").unwrap();
        s.write_string(1, 1, "名称").unwrap();
        s.write_number(2, 0, 1001.0).unwrap();
        s.write_string(2, 1, "苹果").unwrap();
        s.write_number(3, 0, 1002.0).unwrap();
        s.write_string(3, 1, "香蕉").unwrap();
    }
    {
        // 整表全空
        let s = wb.add_worksheet();
        s.set_name("empty").unwrap();
    }
    {
        // 错误值单元格:用 `Formula::set_result` 指定缓存值,rust_xlsxwriter 会写出
        // `t="e"` 单元格(`write_formula("=1/0")` 只会写缓存值 0,不是错误)。
        use rust_xlsxwriter::Formula;
        let s = wb.add_worksheet();
        s.set_name("errors").unwrap();
        s.write_string(0, 0, "h").unwrap();
        s.write_formula(1, 0, Formula::new("1/0").set_result("#DIV/0!"))
            .unwrap();
        s.write_formula(2, 0, Formula::new("NA()").set_result("#N/A"))
            .unwrap();
        s.write_string(3, 0, "x").unwrap();
    }
    {
        // 已用区域不从 A 列开始:前两列整列空(常见于"留白"的表)。
        // 这类表 min_col != 0,能抓出稀疏行展开时忘记减 min_col 的错位。
        let s = wb.add_worksheet();
        s.set_name("offset").unwrap();
        // 数据从 C 列开始(0-based col=2)
        s.write_string(0, 2, "id").unwrap();
        s.write_string(0, 3, "名称").unwrap();
        s.write_string(1, 2, "1").unwrap();
        s.write_string(1, 3, "甲").unwrap();
        s.write_string(2, 2, "2").unwrap();
        s.write_string(2, 3, "乙").unwrap();
        s.write_number(3, 4, 5.0).unwrap(); // 更右的稀疏列
    }
    {
        // 从 B 列开始的单列数据(最小列非 0 且只有一列)
        let s = wb.add_worksheet();
        s.set_name("offset2").unwrap();
        s.write_string(0, 1, "键").unwrap();
        s.write_string(1, 1, "a").unwrap();
        s.write_string(2, 1, "b").unwrap();
    }
    {
        // chartsheet:不是工作表。两条路径都必须给出"空表"而不是报错。
        use rust_xlsxwriter::{Chart, ChartType};
        let mut chart = Chart::new(ChartType::Column);
        chart.add_series().set_values(("basic", 1, 0, 1, 0));
        let c = wb.add_chartsheet();
        c.set_name("chart").unwrap();
        c.insert_chart(0, 0, &chart).unwrap();
    }
    wb.save(path).unwrap();
}

/// 规模用例文件:大一些,用于覆盖"数据行远多于 pending 窗口"
fn make_bulk_workbook(path: &Path, rows: usize, cols: usize) {
    let mut wb = Workbook::new();
    for (index, shape) in ["plain", "text", "mixed"].iter().enumerate() {
        let s = wb.add_worksheet();
        s.set_name(format!("sheet{index}")).unwrap();
        for c in 0..cols {
            s.write_string(0, c as u16, format!("列名{c}")).unwrap();
        }
        for r in 1..rows {
            for c in 0..cols {
                match (*shape, c % 3) {
                    ("plain", _) => {
                        s.write_number(r as u32, c as u16, (r * c) as f64).unwrap();
                    }
                    ("text", _) => {
                        s.write_string(r as u32, c as u16, format!("文本-{r}-{c}"))
                            .unwrap();
                    }
                    (_, 0) => {
                        s.write_number(r as u32, c as u16, r as f64).unwrap();
                    }
                    (_, 1) => {
                        s.write_boolean(r as u32, c as u16, r % 2 == 0).unwrap();
                    }
                    _ => {
                        // 枚举文本:大量重复短串,压 sharedStrings 的去重路径
                        let v = ["华东", "华北", "华南", "西南", "东北"][r % 5];
                        s.write_string(r as u32, c as u16, v).unwrap();
                    }
                }
            }
        }
    }
    wb.save(path).unwrap();
}

fn temp_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("excelookup-stream-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// 逐字段断言两条路径结果一致;不一致时给出定位信息与两个值。
fn assert_paths_agree(
    path: &Path,
    sheet: &str,
    requested: Option<usize>,
    preview: bool,
) -> SheetTable {
    let range = read_sheet_opts_path(path, sheet, requested, preview, ReadPath::Range)
        .unwrap_or_else(|e| panic!("Range 路径读 {sheet} 失败: {e}"));
    let stream = read_sheet_opts_path(path, sheet, requested, preview, ReadPath::Stream)
        .unwrap_or_else(|e| panic!("流式读 {sheet} 失败: {e}"));
    let ctx = format!("sheet={sheet} requested={requested:?} preview={preview}");

    assert_eq!(stream.name, range.name, "[{ctx}] 工作表名不同");
    assert_eq!(stream.headers_of(), range.headers_of(), "[{ctx}] 列名不同");
    // 逐格:整行 Vec<CellValue> 精确相等
    assert_eq!(
        stream.row_count_of(),
        range.row_count_of(),
        "[{ctx}] 行数不同"
    );
    for (index, (a, b)) in stream
        .rows_of()
        .iter()
        .zip(range.rows_of().iter())
        .enumerate()
    {
        if a != b {
            panic!("[{ctx}] 第 {index} 行不同(0-based 数据行)\n  流式 = {a:?}\n  Range = {b:?}");
        }
    }
    assert_eq!(stream.preview, range.preview, "[{ctx}] 顶部预览文本不同");
    assert_eq!(
        stream.preview_non_empty, range.preview_non_empty,
        "[{ctx}] 预览非空标记不同"
    );
    assert_eq!(
        stream.preview_cells, range.preview_cells,
        "[{ctx}] 预览类型化单元格不同"
    );
    assert_eq!(
        stream.auto_header_row, range.auto_header_row,
        "[{ctx}] 自动列名行不同"
    );
    assert_eq!(
        stream.used_header_row, range.used_header_row,
        "[{ctx}] 实际列名行不同"
    );
    assert_eq!(
        stream.first_row_number, range.first_row_number,
        "[{ctx}] 已用区域首行 Excel 行号不同"
    );
    stream
}

/// `SheetTable` 字段都是 pub,这里只是给断言处一组短名字,避免长表达式重复。
trait SheetView {
    fn headers_of(&self) -> &[String];
    fn rows_of(&self) -> &[Vec<excelookup_lib::model::CellValue>];
    fn row_count_of(&self) -> usize;
}

impl SheetView for SheetTable {
    fn headers_of(&self) -> &[String] {
        &self.table.headers
    }
    fn rows_of(&self) -> &[Vec<excelookup_lib::model::CellValue>] {
        &self.table.rows
    }
    fn row_count_of(&self) -> usize {
        self.table.row_count()
    }
}

/// 边界表 × 所有列名行选择(自动 + 越界/空行/数据行/错误行等) × 两种预览开关
#[test]
fn stream_matches_range_on_edge_workbook() {
    let path = temp_path("edge.xlsx");
    make_edge_workbook(&path);

    for sheet in [
        "basic", "titled", "empty", "errors", "offset", "offset2", "chart",
    ] {
        for requested in std::iter::once(None).chain((0..7).map(Some)) {
            for preview in [true, false] {
                assert_paths_agree(&path, sheet, requested, preview);
            }
        }
    }
}

/// 规模表:数据行远多于列名行判定窗口,覆盖"流式已定型后长跑"的路径
#[test]
fn stream_matches_range_on_bulk_workbook() {
    let path = temp_path("bulk.xlsx");
    make_bulk_workbook(&path, 400, 6);

    for sheet in ["sheet0", "sheet1", "sheet2"] {
        for requested in [None, Some(0), Some(1), Some(2), Some(50)] {
            assert_paths_agree(&path, sheet, requested, true);
        }
    }
}

/// 错误值单元格必须被两条路径一致地处理。
///
/// 这是最容易出错的一格:`data_take_cell` 把 `Data::Error` 丢成 `CellValue::Empty`,
/// 但它**参与**"原始非空"判定(预览的 `preview_non_empty` 为 true、列名行不会
/// 被当成空行跳过)。流式实现若把"非空"建立在已转换的 `CellValue` 上,这两处会一起错。
#[test]
fn error_cells_are_dropped_but_count_as_non_empty() {
    let path = temp_path("edge.xlsx");
    make_edge_workbook(&path);

    // 前置:确认错误单元格真的造出来了(否则本测试是空转)
    let probe = read_sheet_opts_path(&path, "errors", None, true, ReadPath::Range).unwrap();
    let has_error_cell = probe
        .table
        .rows
        .iter()
        .flatten()
        .any(|c| matches!(c, excelookup_lib::model::CellValue::Empty));
    assert!(has_error_cell, "造数失败:没有发生错误值→Empty 的丢弃");
    assert!(
        probe.preview_non_empty.iter().skip(1).any(|x| *x),
        "造数失败:错误行未被算作非空(说明没有真正写入错误单元格)"
    );

    for requested in std::iter::once(None).chain((0..4).map(Some)) {
        assert_paths_agree(&path, "errors", requested, true);
    }
}

/// `read_workbook_opts`(首次打开文件的路径)也必须逐格一致。
///
/// 这是 GUI 实际走的那条:整本读时逐表复用同一 handle,且**每张表独立**判定
/// 流式/回退,所以这里比对的是"表数组"而不是单表。
#[test]
fn read_workbook_opts_matches_range_path() {
    let path = temp_path("edge.xlsx");
    make_edge_workbook(&path);

    for header_rows in [
        Vec::new(),
        vec![None, Some(1)],
        vec![Some(0), None, Some(2)],
    ] {
        let opts = ReadOptions {
            header_rows: header_rows.clone(),
            preview: true,
        };
        let stream = read_workbook_opts(&path, opts.clone()).unwrap();
        let range = read_workbook_all_range(&path, opts.clone());
        assert_eq!(
            stream.len(),
            range.len(),
            "表数不同(header_rows={header_rows:?})"
        );
        for (a, b) in stream.iter().zip(range.iter()) {
            let ctx = format!("sheet={} header_rows={header_rows:?}", a.name);
            assert_eq!(a.name, b.name, "[{ctx}] 表名不同");
            assert_eq!(a.table.headers, b.table.headers, "[{ctx}] 列名不同");
            assert_eq!(a.table.rows.len(), b.table.rows.len(), "[{ctx}] 行数不同");
            for (index, (ra, rb)) in a.table.rows.iter().zip(b.table.rows.iter()).enumerate() {
                assert_eq!(ra, rb, "[{ctx}] 第 {index} 行不同");
            }
            assert_eq!(a.preview, b.preview, "[{ctx}] 预览不同");
            assert_eq!(
                a.preview_non_empty, b.preview_non_empty,
                "[{ctx}] 预览非空不同"
            );
            assert_eq!(a.preview_cells, b.preview_cells, "[{ctx}] 预览单元格不同");
            assert_eq!(a.used_header_row, b.used_header_row, "[{ctx}] 列名行不同");
            assert_eq!(
                a.auto_header_row, b.auto_header_row,
                "[{ctx}] 自动列名行不同"
            );
            assert_eq!(a.first_row_number, b.first_row_number, "[{ctx}] 首行号不同");
        }
    }
}

/// 用 `read_sheet_opts_path(Range)` 逐表拼出"整本 Range 路径"的结果,
/// 作为 `read_workbook_opts` 差分测试的参考答案。
fn read_workbook_all_range(
    path: &Path,
    opts: ReadOptions,
) -> Vec<excelookup_lib::read_xlsx::SheetTable> {
    // `read_workbook` 只给 (name, Table), 预览等元信息拿不到,所以逐表走 Range
    let names: Vec<String> = read_workbook(path)
        .unwrap()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let requested = opts.header_rows.get(index).copied().flatten();
            read_sheet_opts_path(path, name, requested, opts.preview, ReadPath::Range).unwrap()
        })
        .collect()
}

/// 自动分派(`ReadPath::Auto`)必须与强制流式结果一致,
/// 否则说明分派逻辑给出了与"用户的文件其实是 xlsx"不符的判断。
#[test]
fn auto_dispatch_uses_stream_for_xlsx() {
    let path = temp_path("edge.xlsx");
    make_edge_workbook(&path);
    for sheet in ["basic", "titled", "empty", "errors"] {
        let auto = read_sheet_opts_path(&path, sheet, None, true, ReadPath::Auto).unwrap();
        let stream = read_sheet_opts_path(&path, sheet, None, true, ReadPath::Stream).unwrap();
        assert_eq!(
            auto.table.rows, stream.table.rows,
            "Auto 分派与流式结果不同(sheet={sheet})"
        );
        assert_eq!(auto.table.headers, stream.table.headers);
        assert_eq!(auto.used_header_row, stream.used_header_row);
    }
}

/// chartsheet 必须得到空表,而不是错误 —— 这是"从 Range 改流式"最容易引入的
/// 行为回归(两条路径对非工作表的语义本来就不一致)。
#[test]
fn chartsheet_reads_as_empty_table_on_both_paths() {
    let path = temp_path("edge.xlsx");
    make_edge_workbook(&path);
    for read_path in [ReadPath::Range, ReadPath::Stream, ReadPath::Auto] {
        let sheet = read_sheet_opts_path(&path, "chart", None, true, read_path)
            .unwrap_or_else(|e| panic!("{read_path:?} 读 chartsheet 报错: {e}"));
        assert!(
            sheet.table.is_empty(),
            "{read_path:?} 读 chartsheet 应得到空表,实际 {} 行",
            sheet.table.row_count()
        );
        assert_eq!(sheet.auto_header_row, None, "{read_path:?}");
    }
}

/// 迟到的左移:表头与首条数据只占 C/D 列,后续数据行突然在更靠左的 B 列出值。
///
/// 首条数据物化时 `min_col=C`;B 列出现后整表最小列左移,已物化的行无法右移
/// 对齐(末尾 `resize` 只能补空)。流式必须回退 `Range` —— 强制 `Stream` 按
/// 接口约定报"无法流式读取",`Auto` 回退后与 `Range` 逐格一致。这是
/// `offset` 用例覆盖不到的形态:`offset` 只向右扩展,这里才是真正触发
/// `Fallback` 设计的场景(该 bug 曾导致首条数据静默错列)。
#[test]
fn late_left_shift_falls_back_to_range() {
    let path = temp_path("late_left_shift.xlsx");
    let mut wb = Workbook::new();
    let s = wb.add_worksheet();
    s.set_name("shift").unwrap();
    s.write_string(0, 2, "id").unwrap();
    s.write_string(0, 3, "姓名").unwrap();
    s.write_number(1, 2, 1.0).unwrap();
    s.write_string(1, 3, "张三").unwrap();
    // 第二条数据突然在更靠左的 B 列出值 → 整表最小列在物化后左移
    s.write_string(2, 1, "X").unwrap();
    s.write_number(2, 2, 2.0).unwrap();
    s.write_string(2, 3, "李四").unwrap();
    wb.save(&path).unwrap();

    // 强制流式:按接口约定报"无法流式读取"
    let err = read_sheet_opts_path(&path, "shift", None, true, ReadPath::Stream)
        .expect_err("左移表应触发流式回退而不是静默错列");
    assert!(
        err.to_string().contains("无法流式读取"),
        "错误文案不符: {err}"
    );

    // Auto 回退后与 Range 逐字段一致
    let auto = read_sheet_opts_path(&path, "shift", None, true, ReadPath::Auto).unwrap();
    let range = read_sheet_opts_path(&path, "shift", None, true, ReadPath::Range).unwrap();
    assert_eq!(auto.table.headers, range.table.headers, "列名不同");
    assert_eq!(auto.table.rows, range.table.rows, "数据行不同");
    assert_eq!(auto.preview, range.preview, "预览文本不同");
    assert_eq!(auto.preview_cells, range.preview_cells, "预览单元格不同");
    assert_eq!(
        auto.preview_non_empty, range.preview_non_empty,
        "预览非空标记不同"
    );
    assert_eq!(auto.used_header_row, range.used_header_row, "实际列名行不同");
    assert_eq!(auto.auto_header_row, range.auto_header_row, "自动列名行不同");
    assert_eq!(
        auto.first_row_number, range.first_row_number,
        "首行号不同"
    );

    // 钉死错列这一失效形态:Range 口径下首条数据的 B 列是空,值从 C 列起
    assert_eq!(auto.table.cell(0, 0), Some(&CellValue::Empty));
    assert_eq!(auto.table.cell(0, 1), Some(&CellValue::Number(1.0)));
    assert_eq!(
        auto.table.cell(0, 2),
        Some(&CellValue::Text("张三".into()))
    );
}

/// 未知扩展名 / 无扩展名的 xlsx 也要被判成流式路径(内容探测,不看扩展名)。
/// 这是"按扩展名分派"会漏掉的场景。
#[test]
fn content_detection_not_extension() {
    let source = temp_path("edge.xlsx");
    make_edge_workbook(&source);
    let no_ext = temp_path("no_extension_file");
    let odd_ext = temp_path("odd.weird");
    std::fs::copy(&source, &no_ext).unwrap();
    std::fs::copy(&source, &odd_ext).unwrap();

    for path in [&no_ext, &odd_ext] {
        let auto = read_sheet_opts_path(path, "basic", None, true, ReadPath::Auto).unwrap();
        let stream = read_sheet_opts_path(path, "basic", None, true, ReadPath::Stream).unwrap();
        assert!(
            !auto.table.is_empty(),
            "{} 应能被内容识别为 xlsx 并读出数据",
            path.display()
        );
        assert_eq!(
            auto.table.rows,
            stream.table.rows,
            "{} 未被分派到流式(或结果不一致)",
            path.display()
        );
    }
}
