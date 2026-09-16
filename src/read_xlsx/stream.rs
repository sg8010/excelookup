//! xlsx 专用流式读表:跳过 calamine 的稠密 `Range<Data>`,由
//! `XlsxCellReader` 逐格直接构建 [`Table`]。
//!
//! **为什么要有这条路径**
//!
//! `Reader::worksheet_range` 会先把整张表展开成稠密的 `Range<Data>`(8B/格枚举
//! 槽位 + 每格独立堆分配),再由调用方转成 `Table`。大表下"稠密矩阵 + 已转换的
//! 部分表"会同时驻留,峰值明显高于最终结果本身。流式则只保留"当前行 + 结果表",
//! 实测 30 万行 × 10 列峰值 208MB → 87MB(-58%),耗时持平。
//!
//! **等价性要求**
//!
//! 本模块必须与 `read_xlsx::table_from_range` 给出**逐格相同**的结果。两侧口径
//! 由 `tests/stream_equivalence.rs` 用差分测试守住(见该文件说明)。
//!
//! 两处历史上就不一致的口径,这里逐字复刻 `read_xlsx` 的现有行为,不做"修正":
//! - 表头文本来自 `data_display`:`Bool` 走 `Display`(小写 `true`)
//! - 数据格来自 `data_take_cell`:`Bool` 显式映射成大写 `TRUE`
//!
//! **回退**
//!
//! 只有一种情况必须放弃流式:某数据行出现比"已见最小列号"更小的列。已输出的
//! 行是定宽(按最小列对齐)的,而整表的最小列此时会左移,宽度口径就错了。这种
//! 畸形表交给调用方重开工作簿走 `Range` 路径。

use std::path::Path;

use anyhow::{Context, Result};
use calamine::{Data, DataRef, Reader, Sheets, XlsxCellReader, XlsxError};

use crate::model::{CellValue, Table};
use crate::read_xlsx::{PREVIEW_ROWS, SheetTable, data_display, data_take_cell};

/// 流式读表的结果:要么成功,要么明确要求回退。
pub(crate) enum StreamRead {
    /// 读成功(与 `table_from_range` 逐格等价)
    Done(SheetTable),
    /// 该工作簿不是 xlsx 系(xls/xlsb/ods),调用方走 `Range` 路径
    Unsupported,
    /// 表对象不是工作表(如 chartsheet):与 `Range` 路径一致地返回空表。
    /// `Range` 路径对这类表返回 `Ok(0×0)`,不能在这里退化成报错。
    NotAWorksheet,
    /// 结构畸形(数据行出现更小列号),调用方重开工作簿走 `Range` 路径
    Fallback,
}

/// 逐格状态:一行在定型前的原始形态。
///
/// 只有「列名行未定」和「预览窗口内」的行需要保留原始 `Data`(用于预览文本与
/// 预览单元格),其余行在 [`add_cell`] 里直接落成 `CellValue`,不产生中间副本。
struct SparseRow {
    row: u32,
    /// 稀疏的定宽源:直接落成的 `CellValue`
    cells: Vec<(u32, CellValue)>,
    /// 原始形态;仅在需要预览/表头判定的行上收集(有界)
    raw: Vec<(u32, Data)>,
    /// 是否已开始收集 `raw`(一旦开始就不能停,否则下标对不上)
    raw_started: bool,
    /// 原始非空判定(`Data::Empty` 之外都算,含错误值)
    non_empty: bool,
}

impl SparseRow {
    fn new(row: u32) -> Self {
        Self {
            row,
            cells: Vec::new(),
            raw: Vec::new(),
            raw_started: false,
            non_empty: false,
        }
    }
}

/// 流式累加器。生命周期与"读一张表"一致。
struct Streamer {
    requested: Option<usize>,
    /// 已用区域(有值单元格的包围盒)
    min_row: Option<u32>,
    max_row: Option<u32>,
    min_col: u32,
    max_col: u32,
    has_value: bool,
    /// 表格行数(非空数据行数,与 `Range` 路径的 `height - header - 1` 同口径)。
    ///
    /// **不能**用 `max_row - min_row` 推:已用区域里的空行会被数据行转换丢弃,
    /// 两者不相等(边界用例里中间就有整行空)。
    table_rows: u32,
    /// 已用区域前 `PREVIEW_ROWS` 行
    head: Vec<SparseRow>,
    header_abs: Option<u32>,
    header_raw: Vec<(u32, Data)>,
    data_rows: Vec<Vec<CellValue>>,
    /// 列名行未定前先缓存的行(有界:见 `pending_cap`)
    pending: Vec<SparseRow>,
    fallback: bool,
    cur: Option<SparseRow>,
}

impl Streamer {
    fn new(requested: Option<usize>) -> Self {
        Self {
            requested,
            min_row: None,
            max_row: None,
            min_col: u32::MAX,
            max_col: 0,
            has_value: false,
            table_rows: 0,
            head: Vec::new(),
            header_abs: None,
            header_raw: Vec::new(),
            data_rows: Vec::new(),
            pending: Vec::new(),
            fallback: false,
            cur: None,
        }
    }

    /// 列名行确定前最多缓存这么多行。
    ///
    /// 指定的列名行相对已用区域首行偏移 `k`,所以目标行最晚在第 `k+1` 行出现;
    /// 再多看到一行仍没等到,就说明该行超出已用区域 → 回退自动。
    fn pending_cap(&self) -> usize {
        self.requested.map_or(2, |k| k + 2)
    }

    /// 本行是否需要保留原始 `Data`(列名行未定,或还在预览窗口内)
    fn need_raw(&self) -> bool {
        self.header_abs.is_none() || self.head.len() < PREVIEW_ROWS
    }

    fn start_row(&mut self, row: u32) {
        if let Some(prev) = self.cur.take() {
            self.finish_row(prev);
        }
        self.cur = Some(SparseRow::new(row));
    }

    fn add_cell(&mut self, col: u32, value: &DataRef<'_>) {
        let non_empty = !matches!(value, DataRef::Empty);
        let row_index = self.cur.as_ref().map_or(0, |r| r.row);
        if non_empty {
            if self.min_row.is_none() {
                self.min_row = Some(row_index);
            }
            self.max_row = Some(row_index);
            self.min_col = self.min_col.min(col);
            self.max_col = self.max_col.max(col);
            self.has_value = true;
        }

        let need_raw = self.need_raw();
        let Some(cur) = self.cur.as_mut() else {
            return;
        };
        cur.non_empty |= non_empty;
        if need_raw && non_empty {
            cur.raw_started = true;
        }
        if cur.raw_started {
            cur.raw.push((col, raw_of(value)));
        }
        cur.cells.push((col, cell_from_ref(value)));
    }

    /// 数据行出现比当前最小列更小的列 → 整表宽度会左移,必须回退
    fn would_shift(&self, row: &SparseRow) -> bool {
        self.has_value
            && row
                .cells
                .iter()
                .any(|(c, v)| *c < self.min_col && !matches!(v, CellValue::Empty))
    }

    fn finish_row(&mut self, row: SparseRow) {
        // 预览窗口:已用区域首行起的前 PREVIEW_ROWS 行(与列名行判定无关)
        if self
            .min_row
            .is_some_and(|min_row| row.row >= min_row && self.head.len() < PREVIEW_ROWS)
        {
            self.head.push(SparseRow {
                row: row.row,
                cells: row.cells.clone(),
                raw: row.raw.clone(),
                raw_started: row.raw_started,
                non_empty: row.non_empty,
            });
        }

        if self.header_abs.is_some() {
            self.push_data(row);
            return;
        }

        // 列名行未定:先缓存,再看目标行是否已到
        self.pending.push(row);
        let target = self
            .requested
            .map(|k| self.min_row.map(|min| min + k as u32))
            .unwrap_or(self.min_row);
        if let Some(pos) =
            target.and_then(|target| self.pending.iter().position(|row| row.row == target))
        {
            let target_row = self.pending.remove(pos);
            if target_row.non_empty {
                self.header_abs = Some(target_row.row);
                self.header_raw = target_row.raw;
            } else {
                // 指定行整行空 → 与 Range 路径一致:回退自动(已用区域首行)
                self.use_auto_header();
            }
        }
        // 已用区域已经走过"指定行"仍没等到它 → 行数不足,回退自动
        if self.header_abs.is_none() && self.pending.len() > self.pending_cap() {
            self.use_auto_header();
        }
        if self.header_abs.is_some() {
            self.flush_pending();
        }
    }

    /// 回退到自动列名行(已用区域首行)。整表无值时置回退标记。
    fn use_auto_header(&mut self) {
        match self.pending.first().or_else(|| self.head.first()) {
            Some(first) => {
                self.header_abs = Some(first.row);
                self.header_raw = first.raw.clone();
            }
            None => self.fallback = true,
        }
    }

    /// 列名行确定后,把缓存中位于列名行之后的行补成数据行
    fn flush_pending(&mut self) {
        let header = self.header_abs.expect("调用前已确定列名行");
        let pending = std::mem::take(&mut self.pending);
        for row in pending {
            if row.row > header {
                self.push_data(row);
            }
        }
    }

    /// 数据行:先查宽度左移,再按已知宽度展开
    fn push_data(&mut self, row: SparseRow) {
        if self.would_shift(&row) {
            self.fallback = true;
            self.data_rows.clear();
            return;
        }
        if !row.non_empty {
            return;
        }
        self.table_rows = self.table_rows.saturating_add(1);
        let width = (self.max_col - self.min_col + 1) as usize;
        let mut out = vec![CellValue::Empty; width];
        for (col, value) in row.cells {
            let index = col.saturating_sub(self.min_col) as usize;
            if index < out.len() {
                out[index] = value;
            }
        }
        self.data_rows.push(out);
    }

    /// 稀疏原始行 → 预览文本行(与 `data_display` 同口径)
    fn expand_text(cells: &[(u32, Data)], min_col: u32, width: usize) -> Vec<String> {
        let mut out = vec![String::new(); width];
        for (col, value) in cells {
            let index = col.saturating_sub(min_col) as usize;
            if index < out.len() {
                out[index] = data_display(value);
            }
        }
        out
    }

    /// 稀疏原始行 → 类型化单元格行(与 `data_take_cell` 同口径)
    fn expand_cells(cells: &[(u32, Data)], min_col: u32, width: usize) -> Vec<CellValue> {
        let mut out = vec![CellValue::Empty; width];
        for (col, value) in cells {
            let index = col.saturating_sub(min_col) as usize;
            if index < out.len() {
                out[index] = data_take_cell(&mut value.clone());
            }
        }
        out
    }

    /// 预览:已用区域前 PREVIEW_ROWS 行(缺号补空行)
    ///
    /// 条数与 `Range` 路径一致 —— `Range` 取 `min(PREVIEW_ROWS, 工作表的已用区域
    /// 行高)`,其中行高是"行数"而不是"结尾绝对行号"。这里用 `max_row - min_row + 1`
    /// 正是同一个量;不能只数已收集的 `head`(那会在行数不足时给出过少的条数)。
    fn preview_height(&self, min_row: u32) -> usize {
        let max_row = self.max_row.unwrap_or(min_row);
        (max_row - min_row + 1) as usize
    }

    fn finish(mut self, preview_wanted: bool) -> StreamRead {
        if let Some(cur) = self.cur.take() {
            self.finish_row(cur);
        }
        if self.fallback {
            return StreamRead::Fallback;
        }
        let Some(min_row) = self.min_row else {
            // 整表全空(与 Range 路径一致:无列名行、无行)
            return StreamRead::Done(SheetTable::default());
        };
        if self.header_abs.is_none() {
            // 指定列名行超出已用区域行数 → 与 Range 路径一致:回退自动
            self.use_auto_header();
            if self.fallback {
                return StreamRead::Fallback;
            }
            self.flush_pending();
        }
        let Some(header_abs) = self.header_abs else {
            return StreamRead::Done(SheetTable::default());
        };

        let min_col = self.min_col;
        let width = (self.max_col - min_col + 1) as usize;

        // 列名:空列名补「列N」,重名加序号 —— 与 Range 路径同口径
        let mut headers = Self::expand_text(&self.header_raw, min_col, width);
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

        let mut table = Table::new(headers);
        table.rows.reserve(self.data_rows.len());
        for mut row in std::mem::take(&mut self.data_rows) {
            row.resize(width, CellValue::Empty);
            table.rows.push(row);
        }

        // 预览:已用区域前 PREVIEW_ROWS 行(缺号补空行,高度按已用区域截断)
        let (preview, preview_cells, preview_non_empty) = if preview_wanted {
            let used_height = self.preview_height(min_row);
            let mut preview = Vec::with_capacity(used_height.min(PREVIEW_ROWS));
            let mut preview_cells = Vec::with_capacity(used_height.min(PREVIEW_ROWS));
            let mut preview_non_empty = Vec::with_capacity(used_height.min(PREVIEW_ROWS));
            for offset in 0..used_height.min(PREVIEW_ROWS) {
                let want = min_row + offset as u32;
                let found = self.head.iter().find(|row| row.row == want);
                let (cells, non_empty) = match found {
                    Some(row) => (row.raw.as_slice(), row.non_empty),
                    None => (&[][..], false),
                };
                preview_non_empty.push(non_empty);
                preview.push(Self::expand_text(cells, min_col, width));
                preview_cells.push(Self::expand_cells(cells, min_col, width));
            }
            (preview, preview_cells, preview_non_empty)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };

        StreamRead::Done(SheetTable {
            name: String::new(),
            table,
            preview,
            preview_cells,
            preview_non_empty,
            auto_header_row: Some(0),
            used_header_row: Some((header_abs - min_row) as usize),
            first_row_number: min_row as usize + 1,
        })
    }
}

/// 流式读一个 xlsx 工作表。
///
/// 会自己打开工作簿;批量读多表时用 [`read_sheet_with_workbook`] 复用 handle
/// 更省(打开工作簿要解析 sharedStrings,大表上不是免费操作)。
pub(crate) fn read_sheet_stream(
    path: &Path,
    sheet_name: &str,
    requested: Option<usize>,
    preview: bool,
) -> Result<StreamRead> {
    let mut workbook = calamine::open_workbook_auto(path)
        .with_context(|| format!("无法打开文件: {}", path.display()))?;
    read_sheet_with_workbook(&mut workbook, sheet_name, requested, preview)
}

/// 用**已打开**的工作簿流式读一张表。
///
/// 复用同一个 handle 逐表读,避免为每张表重新 open(open 要解析 sharedStrings,
/// 大表上单次就要几十 ms)。`XlsxCellReader` 会持有 `&mut Xlsx`,所以逐表顺序读。
pub(crate) fn read_sheet_with_workbook<R: std::io::Read + std::io::Seek>(
    workbook: &mut Sheets<R>,
    sheet_name: &str,
    requested: Option<usize>,
    preview: bool,
) -> Result<StreamRead> {
    let Sheets::Xlsx(xlsx) = workbook else {
        return Ok(StreamRead::Unsupported);
    };
    let reader = match xlsx.worksheet_cells_reader(sheet_name) {
        Ok(reader) => reader,
        Err(XlsxError::NotAWorksheet(_)) => return Ok(StreamRead::NotAWorksheet),
        Err(error) => {
            return Err(anyhow::Error::new(error))
                .with_context(|| format!("读取工作表「{sheet_name}」失败"));
        }
    };
    consume(reader, requested, preview)
}

/// 驱动一个单元格读取器直到流结束
fn consume<R: std::io::Read + std::io::Seek>(
    mut reader: XlsxCellReader<'_, R>,
    requested: Option<usize>,
    preview: bool,
) -> Result<StreamRead> {
    let mut streamer = Streamer::new(requested);
    let mut current_row: Option<u32> = None;
    loop {
        match reader.next_cell() {
            Ok(Some(cell)) => {
                let (row, col) = cell.get_position();
                if current_row != Some(row) {
                    streamer.start_row(row);
                    current_row = Some(row);
                }
                streamer.add_cell(col, cell.get_value());
            }
            Ok(None) => break,
            Err(error) => {
                return Err(anyhow::Error::new(error)).context("流式读取单元格失败");
            }
        }
    }
    Ok(streamer.finish(preview))
}

/// `DataRef` → `CellValue`。
///
/// 与 `read_xlsx::data_take_cell` 同口径,但**不做中转 `Data`**:引用型共享
/// 字符串只在这里分配一次。
fn cell_from_ref(value: &DataRef<'_>) -> CellValue {
    match value {
        DataRef::Int(i) => CellValue::Number(*i as f64),
        DataRef::Float(f) => CellValue::Number(*f),
        DataRef::String(s) => CellValue::Text(s.clone()),
        DataRef::SharedString(s) => CellValue::Text((*s).to_owned()),
        DataRef::Bool(b) => CellValue::Text(if *b { "TRUE" } else { "FALSE" }.into()),
        DataRef::DateTime(dt) => CellValue::Text(dt.to_string()),
        DataRef::DateTimeIso(s) | DataRef::DurationIso(s) => CellValue::Text(s.clone()),
        // 与 data_take_cell 一致:错误单元格丢弃为 Empty
        DataRef::Error(_) => CellValue::Empty,
        DataRef::Empty => CellValue::Empty,
    }
}

/// `DataRef` → `Data`(仅受限行需要,有界)
fn raw_of(value: &DataRef<'_>) -> Data {
    match value {
        DataRef::Int(v) => Data::Int(*v),
        DataRef::Float(v) => Data::Float(*v),
        DataRef::String(v) => Data::String(v.clone()),
        DataRef::SharedString(v) => Data::String((*v).to_owned()),
        DataRef::Bool(v) => Data::Bool(*v),
        DataRef::DateTime(v) => Data::DateTime(*v),
        DataRef::DateTimeIso(v) => Data::DateTimeIso(v.clone()),
        DataRef::DurationIso(v) => Data::DurationIso(v.clone()),
        DataRef::Error(v) => Data::Error(v.clone()),
        DataRef::Empty => Data::Empty,
    }
}

/// 让 `Reader` trait 的导入在无 `_` 前缀调用时也成立(供未来扩展)
#[allow(dead_code)]
fn _assert_reader_trait<R: std::io::Read + std::io::Seek, T: Reader<R>>(_: &T) {}
