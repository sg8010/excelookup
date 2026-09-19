//! Join 引擎:实现 VLOOKUP(left join)与 inner join
//!
//! 左右表分别选择输出列;A 匹配列默认带出、B 匹配列默认不带出,两侧至少保留一列匹配列在输出中(由 UI 与 worker 用 has_key_output 把关,join 本身不强制)。
//! 键支持多列复合;归一化可配置:数字/文本互认+trim、中文/英文括号互认、案号分支后缀忽略。

#[cfg(test)]
use std::borrow::Cow;
use std::collections::{HashMap, hash_map::Entry};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

// B 索引每行插一次、A 表每行查一次,百万行规模下哈希本身占 join 耗时可观。
// SipHash 为抗 HashDoS 而设计,读本地 Excel 文件用不到这层防护,改用 hashbrown
// 默认的 foldhash。map 迭代顺序会变,但输出顺序由 A 表行序决定,不依赖迭代。
use foldhash::fast::RandomState;

use crate::model::{CellValue, Table};

static EMPTY_CELL: CellValue = CellValue::Empty;

/// join 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    /// VLOOKUP 语义:保留左表所有行,匹配不到则右侧补空
    Left,
    /// 只保留两表都能匹配的行
    Inner,
}

impl JoinType {
    pub fn label(&self) -> &'static str {
        match self {
            JoinType::Left => "左连接（VLOOKUP）",
            JoinType::Inner => "内连接（交集）",
        }
    }
    pub fn all() -> [JoinType; 2] {
        [JoinType::Left, JoinType::Inner]
    }

    /// 语义说明:每种连接类型对 A/B 两侧行的去留
    pub fn hint(&self) -> &'static str {
        match self {
            JoinType::Left => "保留 A 的全部行",
            JoinType::Inner => "仅保留 A、B 都能匹配到的行",
        }
    }
}

/// 键归一化策略(独立开关,可任意组合)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyMode {
    /// 宽松匹配:数字 1 与文本 "1" 视为相同,并 trim 首尾空白
    pub number_text: bool,
    /// 括号归一化:中文括号(（）【】｛｝)与英文括号([]{})互认
    pub brackets: bool,
    /// 忽略紧接“号”的末尾中文数字分支后缀，如之一、（之十二）。
    pub case_suffix: bool,
}

impl KeyMode {
    /// 全部关闭:数字 1 与文本 "1" 视为不同,括号不归一化
    pub const EXACT: Self = Self {
        number_text: false,
        brackets: false,
        case_suffix: false,
    };
    /// 仅数字/文本互认 + trim(不带括号归一化)
    pub const NORMALIZE: Self = Self {
        number_text: true,
        brackets: false,
        case_suffix: false,
    };
}

impl Default for KeyMode {
    fn default() -> Self {
        // 默认开启数字/文本互认与括号归一化；案号专用清理需显式启用。
        Self {
            number_text: true,
            brackets: true,
            case_suffix: false,
        }
    }
}

/// join 参数
#[derive(Debug, Clone)]
pub struct JoinSpec {
    pub join_type: JoinType,
    /// 左表键列下标(多列 = 复合键)
    pub left_keys: Vec<usize>,
    /// 右表键列下标
    pub right_keys: Vec<usize>,
    /// 左表输出列下标。None 表示沿用默认(输出左表全部列),与「Some 恰好列出全部
    /// 列」行为完全等价;Some 空列表表示左表一列都不输出。越界项在 `run_join` 里
    /// 按左表实际列数丢弃。
    pub left_pick: Option<Vec<usize>>,
    /// 右表取值列下标(结果中仅这些列来自右表)
    pub right_pick: Vec<usize>,
    pub key_mode: KeyMode,
    /// 右表同键多行时是否全部展开。true=逐行展开(VLOOKUP 的重复键也取全);
    /// false=只取第一条(经典 VLOOKUP 语义),避免结果行数爆炸。
    pub expand_dup: bool,
}

impl JoinSpec {
    /// 解析出真正生效的左表输出列:越界下标按左表列数丢弃,顺序沿用调用方给定的顺序。
    ///
    /// `None`(默认)与「列出全部列」得到同一结果,`Some(&[])` 得到空列表。
    pub fn resolved_left_pick(&self, left_col_count: usize) -> Vec<usize> {
        resolve_left_pick(self.left_pick.as_deref(), left_col_count)
    }

    /// 结果表是否至少有一列输出。判据与 UI 共用 [`has_output_columns`]。
    pub fn has_output_columns(&self, left_col_count: usize, right_col_count: usize) -> bool {
        has_output_columns(
            self.left_pick.as_deref(),
            &self.right_pick,
            left_col_count,
            right_col_count,
        )
    }

    /// 输出里是否至少保留了一列匹配列。判据与 UI 共用 [`has_key_output`]。
    pub fn has_key_output(&self, left_col_count: usize, right_col_count: usize) -> bool {
        has_key_output(
            &self.left_keys,
            &self.right_keys,
            self.left_pick.as_deref(),
            &self.right_pick,
            left_col_count,
            right_col_count,
        )
    }
}

/// 解析左表输出列:越界下标按左表实际列数丢弃,顺序沿用调用方给定的顺序。
///
/// `None`(默认)展开成「左表全部列」。
pub fn resolve_left_pick(left_pick: Option<&[usize]>, left_col_count: usize) -> Vec<usize> {
    match left_pick {
        None => (0..left_col_count).collect(),
        Some(columns) => columns
            .iter()
            .copied()
            .filter(|&column| column < left_col_count)
            .collect(),
    }
}

/// 结果表是否至少有一列输出。
///
/// 左右输出列都为空时 Join 仍能算出命中,但结果没有任何可看/可导出的列。UI 的
/// 「执行连接」按钮与后台线程必须用同一个判据,否则会出现「按钮可点、点了却报错」。
/// 越界下标也在这里一并丢弃,调用方不要各自再过滤一遍。
pub fn has_output_columns(
    left_pick: Option<&[usize]>,
    right_pick: &[usize],
    left_col_count: usize,
    right_col_count: usize,
) -> bool {
    let left_selected = match left_pick {
        None => left_col_count > 0,
        Some(columns) => columns.iter().any(|&column| column < left_col_count),
    };
    left_selected
        || right_pick
            .iter()
            .any(|&column| column < right_col_count)
}

/// 输出里是否至少保留了一列匹配列(A 匹配列或 B 匹配列)。
///
/// A 侧按 [`resolve_left_pick`] 解析后判断;B 侧只认 `right_pick` 里的合法下标。
/// 越界的键列下标视为未输出。UI 的「执行连接」按钮与后台线程共用此判据。
pub fn has_key_output(
    left_keys: &[usize],
    right_keys: &[usize],
    left_pick: Option<&[usize]>,
    right_pick: &[usize],
    left_col_count: usize,
    right_col_count: usize,
) -> bool {
    let left_out = resolve_left_pick(left_pick, left_col_count);
    left_keys.iter().any(|k| left_out.contains(k))
        || right_keys
            .iter()
            .any(|k| *k < right_col_count && right_pick.contains(k))
}

/// `JoinedRow::right_row` 的未命中哨兵。
///
/// Excel 单表最多 1,048,576 行,`u32` 足够表示源行号;用哨兵而不是
/// `Option<u32>` 是为了让 `JoinedRow` 保持 8 字节——结果行数可达 500 万,
/// 每行 4 字节的差距就是 20MB。
pub const UNMATCHED_RIGHT_ROW: u32 = u32::MAX;

/// Join 结果中的一行引用。
///
/// 结果不再复制 A/B 的单元格,而是记录输出行对应的源数据行。`right_row` 等于
/// [`UNMATCHED_RIGHT_ROW`] 表示左连接未命中,输出的 B 字段按空值处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinedRow {
    pub left_row: u32,
    pub right_row: u32,
}

/// 基于源表行号的 Join 结果视图。
///
/// `headers` 和列映射很小;真正的数据仍保存在传入 Join 的 A/B 表中。调用方需要
/// 在视图存活期间保留这两张源表,并通过 [`JoinedTable::cell`] 读取单元格。
#[derive(Debug, Clone, Default)]
pub struct JoinedTable {
    /// 输出列名:A 表选中的列 + B 表选中的列。
    pub headers: Vec<String>,
    /// 每个输出行对应的 A/B 源行号。
    pub rows: Vec<JoinedRow>,
    /// 输出中每个 A 列对应的源列号,顺序与 headers 的左半部分一致;其长度即输出中
    /// 的 A 列数,不再单独保存一份冗余计数。
    pub left_pick: Vec<usize>,
    /// 输出中每个 B 列对应的源列号,顺序与 headers 的右半部分一致。
    pub right_pick: Vec<usize>,
}

impl JoinedTable {
    pub fn col_count(&self) -> usize {
        self.headers.len()
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// 该输出行是否命中。左连接未命中的 A 行(右侧补空)为 false,内连接全为
    /// true;重复键展开出的多行都算命中。
    pub fn row_hit(&self, row: usize) -> bool {
        self.rows
            .get(row)
            .is_some_and(|joined_row| joined_row.right_row != UNMATCHED_RIGHT_ROW)
    }

    /// 读取输出视图中的单元格。
    ///
    /// 未命中的 B 单元格返回 `Some(CellValue::Empty)`,保持与物化结果的语义一致;
    /// 源表中本身的空单元格也会返回 `Some(CellValue::Empty)`。
    pub fn cell<'a>(
        &self,
        left: &'a Table,
        right: &'a Table,
        row: usize,
        column: usize,
    ) -> Option<&'a CellValue> {
        let joined_row = self.rows.get(row)?;
        if column < self.left_pick.len() {
            let left_column = *self.left_pick.get(column)?;
            return Some(
                left.rows
                    .get(joined_row.left_row as usize)
                    .and_then(|source_row| source_row.get(left_column))
                    .unwrap_or(&EMPTY_CELL),
            );
        }

        let right_column = *self.right_pick.get(column - self.left_pick.len())?;
        if joined_row.right_row == UNMATCHED_RIGHT_ROW {
            return Some(&EMPTY_CELL);
        }
        Some(
            right
                .rows
                .get(joined_row.right_row as usize)
                .and_then(|source_row| source_row.get(right_column))
                .unwrap_or(&EMPTY_CELL),
        )
    }

    /// 按需物化为独立 Table。GUI 和大数据导出不应调用此方法,仅适合需要独立
    /// 数据快照的调用方;它会重新产生完整结果数据。
    pub fn materialize(&self, left: &Table, right: &Table) -> Table {
        let mut table = Table::new(self.headers.clone());
        table.rows.reserve(self.rows.len());
        for row in 0..self.rows.len() {
            let mut cells = Vec::with_capacity(self.col_count());
            for column in 0..self.col_count() {
                cells.push(
                    self.cell(left, right, row, column)
                        .cloned()
                        .unwrap_or(CellValue::Empty),
                );
            }
            table.rows.push(cells);
        }
        table
    }
}

/// join 结果
#[derive(Debug, Clone)]
pub struct JoinResult {
    /// 输出视图:左表选中列 + 右表取值列,只保存源行引用。
    pub table: JoinedTable,
    pub left_total: usize,
    pub left_matched: usize,
    pub right_total: usize,
    /// 右表行中被匹配过的行数
    pub right_matched_rows: usize,
    pub out_rows: usize,
    /// 输出行中命中的行数(结果表口径;重复键展开时可能大于 `left_matched`)。
    /// 未命中行数 = `out_rows - matched_rows`,不再单独保存平行数组。
    pub matched_rows: usize,
}

/// 连接输出行数的预估结果。
///
/// `output_rows` 与真正 `join` 的输出表行数口径一致：Left 会把未命中的
/// A 行各算 1 行，Inner 不算；重复键是否展开由调用参数决定。计数饱和
/// 到 `usize::MAX`，因此预估只用于上限判断时不会发生整数回绕。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinEstimate {
    pub output_rows: usize,
    pub max_dup: usize,
    pub distinct_keys: usize,
}

/// Join 分阶段计时，仅供性能诊断/benchmark 使用。
///
/// `preflight` 是带上限连接在真正生成结果引用前做的 A 侧预估扫描；`probe` 与
/// `materialize` 是真正输出引用扫描中的两个部分。普通 `join` 不采集这些计时。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JoinTimings {
    pub index: Duration,
    pub preflight: Duration,
    pub probe: Duration,
    pub materialize: Duration,
    pub total: Duration,
    pub estimate: Option<JoinEstimate>,
}

/// 输出上限拒绝信息；包含预估值和拒绝路径已经发生的分阶段耗时。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinLimitExceeded {
    pub estimate: JoinEstimate,
    pub timings: JoinTimings,
}

/// 右表键对应的行集合。
///
/// 唯一键不再为单元素 `Vec` 分配；只有真正遇到重复键且开启展开时才
/// 升级为 `Many`。关闭展开时 `One` 只保留首行，`count` 仅用于诊断最大
/// 重复数，不保存其余行号。
#[derive(Debug)]
enum RowMatches {
    One { row: usize, count: usize },
    Many(Vec<usize>),
}

impl RowMatches {
    fn total_count(&self) -> usize {
        match self {
            RowMatches::One { count, .. } => *count,
            RowMatches::Many(rows) => rows.len(),
        }
    }

    fn selected_count(&self, expand_dup: bool) -> usize {
        if expand_dup { self.total_count() } else { 1 }
    }

    fn for_each_selected(&self, expand_dup: bool, mut f: impl FnMut(usize)) {
        match self {
            RowMatches::One { row, .. } => f(*row),
            RowMatches::Many(rows) => {
                for &row in rows.iter().take(if expand_dup { rows.len() } else { 1 }) {
                    f(row);
                }
            }
        }
    }
}

/// 右表索引。一次构建后可先做输出行数预估，再复用于真正 Join。
struct JoinIndex {
    rows: HashMap<String, RowMatches, RandomState>,
    max_dup: usize,
    expand_dup: bool,
    mode: KeyMode,
}

impl JoinIndex {
    fn build(right: &Table, rk: &[usize], mode: KeyMode, expand_dup: bool) -> Self {
        // 不按 B 总行数无条件 reserve：高重复数据的 distinct key 数可能远小于
        // B 行数，HashMap 按需增长能避免先分配一大块空桶。
        let mut rows: HashMap<String, RowMatches, RandomState> = HashMap::default();
        let mut max_dup = 0usize;

        for (i, row) in right.rows.iter().enumerate() {
            let Some(key) = make_key(row, rk, mode) else {
                continue;
            };
            match rows.entry(key) {
                Entry::Vacant(slot) => {
                    slot.insert(RowMatches::One { row: i, count: 1 });
                    max_dup = max_dup.max(1);
                }
                Entry::Occupied(slot) => {
                    let entry = slot.into_mut();
                    if expand_dup {
                        match entry {
                            RowMatches::One { row, .. } => {
                                let first = *row;
                                *entry = RowMatches::Many(vec![first, i]);
                            }
                            RowMatches::Many(rows) => rows.push(i),
                        }
                    } else if let RowMatches::One { count, .. } = entry {
                        // 不保存重复行号，但仍保留饱和的重复计数供诊断使用。
                        *count = count.saturating_add(1);
                    }
                    max_dup = max_dup.max(entry.total_count());
                }
            }
        }

        Self {
            rows,
            max_dup,
            expand_dup,
            mode,
        }
    }

    fn lookup(&self, key: &str) -> Option<&RowMatches> {
        self.rows.get(key)
    }
}

/// 中文括号 → 对应英文括号(括号归一化)。没有可替换字符时直接借用原文。
#[cfg(test)]
fn fold_brackets(s: &str) -> Cow<'_, str> {
    if !s.chars().any(is_foldable_bracket) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(s.chars().map(|c| fold_bracket(c).unwrap_or(c)).collect())
}

fn fold_bracket(c: char) -> Option<char> {
    match c {
        '（' => Some('('),
        '）' => Some(')'),
        '【' => Some('['),
        '】' => Some(']'),
        '｛' => Some('{'),
        '｝' => Some('}'),
        '〔' => Some('('),
        '〕' => Some(')'),
        _ => None,
    }
}

#[cfg(test)]
fn is_foldable_bracket(c: char) -> bool {
    fold_bracket(c).is_some()
}

/// 把文本正文直接追加到目标 key。trim 只借用原字符串切片；括号归一化
/// 发现替换时也直接写入目标缓冲区，不创建中间 String。
fn append_folded_text(out: &mut String, text: &str) {
    let mut segment_start = 0;
    for (offset, c) in text.char_indices() {
        let Some(folded) = fold_bracket(c) else {
            continue;
        };
        out.push_str(&text[segment_start..offset]);
        out.push(folded);
        segment_start = offset + c.len_utf8();
    }
    out.push_str(&text[segment_start..]);
}

/// 只借用基础案号切片，不修改原值。仅接受末尾中文数字，括号必须成对；
/// 不解析数字大小或校验中文数字语法，也不处理阿拉伯数字与复合后缀。
fn strip_case_suffix(text: &str) -> &str {
    let (body, opening) = if let Some(body) = text.strip_suffix('）') {
        (body, Some('（'))
    } else if let Some(body) = text.strip_suffix(')') {
        (body, Some('('))
    } else {
        (text, None)
    };
    let prefix = body.trim_end_matches(|c: char| "零〇一二三四五六七八九十百千万两".contains(c));
    if prefix.len() == body.len() {
        return text;
    }
    let Some(mut base) = prefix.strip_suffix('之') else {
        return text;
    };
    if let Some(opening) = opening {
        let Some(unwrapped) = base.strip_suffix(opening) else {
            return text;
        };
        base = unwrapped;
    }
    if base.ends_with('号') { base } else { text }
}

/// 向目标 key 追加一个带类型前缀的片段，返回该片段是否非空。
///
/// 单列路径会直接调用这个函数，不经过 `Vec<String> + join`。正文 trim
/// 借用切片，前缀仍使右表索引中的规范 key 必须拥有自己的字符串；A 表
/// 则把它写进复用的 probe 缓冲区。
fn append_key_part(out: &mut String, value: &CellValue, mode: KeyMode) -> bool {
    match value {
        CellValue::Empty => false,
        CellValue::Number(n) => {
            let prefix = if mode.number_text { "V:" } else { "N:" };
            out.reserve(24);
            if n.fract() == 0.0 {
                write!(out, "{prefix}{n:.0}").expect("写入 String 不会失败");
            } else {
                write!(out, "{prefix}{n}").expect("写入 String 不会失败");
            }
            true
        }
        CellValue::Text(s) => {
            let body = if mode.number_text { s.trim() } else { s };
            let body = if mode.case_suffix {
                strip_case_suffix(body)
            } else {
                body
            };
            let prefix = if mode.number_text { "V:" } else { "S:" };
            out.reserve(prefix.len() + body.len());
            out.push_str(prefix);
            if mode.brackets {
                append_folded_text(out, body);
            } else {
                out.push_str(body);
            }
            true
        }
    }
}

/// 将一行规范化为 key。调用者拥有并复用 `key`，本函数在每行开始时清空
/// 它；任何列越界或整键为空的失败路径也会清空，避免把上一行的内容带入
/// 下一次 HashMap 查询。
///
/// 复合键仍使用历史 U+0001 分隔符且不转义；因此原有编码碰撞契约保持不变。
fn write_key(row: &[CellValue], cols: &[usize], mode: KeyMode, key: &mut String) -> bool {
    key.clear();
    let valid = match cols {
        [] => false,
        [col] => row
            .get(*col)
            .is_some_and(|value| append_key_part(key, value, mode)),
        _ => {
            let mut any_non_empty = false;
            let mut all_columns_present = true;
            for (i, &col) in cols.iter().enumerate() {
                let Some(value) = row.get(col) else {
                    all_columns_present = false;
                    break;
                };
                if i != 0 {
                    key.push('\u{1}');
                }
                any_non_empty |= append_key_part(key, value, mode);
            }
            all_columns_present && any_non_empty
        }
    };
    if !valid {
        // 单列 Empty、全 Empty 复合键和部分写入后越界都必须留下空缓冲区。
        key.clear();
    }
    valid
}

/// 为右表索引生成拥有所有权的 key；A 表的预估/查询路径改用 `write_key`
/// 复用单个缓冲区，避免逐行创建 String。
fn make_key(row: &[CellValue], cols: &[usize], mode: KeyMode) -> Option<String> {
    let mut key = String::new();
    write_key(row, cols, mode, &mut key).then_some(key)
}

/// 预估连接输出行数与 B 侧重复键诊断。
///
/// `output_rows` 与对应 `join` 的真实输出行数一致：Left 会计入每个未
/// 命中的 A 行，Inner 不计入；`expand_dup=false` 时每个命中键只计 1 行。
/// 返回的计数在溢出时饱和到 `usize::MAX`。
pub fn estimate_join_rows(
    left: &Table,
    right: &Table,
    lk: &[usize],
    rk: &[usize],
    mode: KeyMode,
    join_type: JoinType,
    expand_dup: bool,
) -> JoinEstimate {
    let index = JoinIndex::build(right, rk, mode, expand_dup);
    let mut key = String::new();
    index.estimate(left, lk, join_type, &mut key)
}

impl JoinIndex {
    fn estimate(
        &self,
        left: &Table,
        lk: &[usize],
        join_type: JoinType,
        key: &mut String,
    ) -> JoinEstimate {
        let mut output_rows = 0usize;
        for row in &left.rows {
            let matches = if write_key(row, lk, self.mode, key) {
                self.lookup(key)
            } else {
                None
            };
            match matches {
                Some(matches) => {
                    output_rows =
                        add_output_rows(output_rows, matches.selected_count(self.expand_dup));
                }
                None if join_type == JoinType::Left => {
                    output_rows = add_output_rows(output_rows, 1);
                }
                None => {}
            }
        }
        JoinEstimate {
            output_rows,
            max_dup: self.max_dup,
            distinct_keys: self.rows.len(),
        }
    }
}

#[inline]
fn add_output_rows(total: usize, additional: usize) -> usize {
    total.saturating_add(additional)
}

fn run_join(
    left: &Table,
    right: &Table,
    spec: &JoinSpec,
    max_output_rows: Option<usize>,
    collect_timings: bool,
) -> Result<(JoinResult, JoinTimings), JoinLimitExceeded> {
    // 结果行号用 u32 存放;源表真的超过 u32::MAX 行时下面的 `as u32` 会静默
    // 截断成错误引用,所以入口直接拒绝。Excel 单表上限约 104 万行,仅防御。
    assert!(
        left.rows.len() <= u32::MAX as usize && right.rows.len() <= u32::MAX as usize,
        "源表行数超过 u32 行号可表示的上限"
    );

    let started = Instant::now();
    let index_started = Instant::now();
    let index = JoinIndex::build(right, &spec.right_keys, spec.key_mode, spec.expand_dup);
    let mut timings = JoinTimings {
        index: index_started.elapsed(),
        ..JoinTimings::default()
    };
    // 预估与实际 probe 共用这一个 A 侧 key 缓冲区；write_key 每行开始和
    // 失败时都会清空它，容量增长仍可能分配，但不会逐行新建 String。
    let mut left_key = String::new();

    // 有上限时先判断是否可能超限:任一 A 行最多命中「B 单键最大重复数」个右行
    // (Left 下未命中的 A 行也只占 1 行),所以 left_rows × max(1, max_dup) 是
    // 输出行数的可靠上界,而这个 max_dup 是索引构建时顺手统计的,不用扫 A 表。
    // 上界不超过上限就必定通过,直接跳过对整张 A 表的预估扫描(那一遍的键
    // 规范化开销与 probe 一遍相当);上界超过上限才走原预估流程,拒绝路径的
    // 耗时、内存与精确诊断都不变,`timings.estimate` 也只在真正预估时填充。
    let estimate_needed = max_output_rows
        .is_some_and(|limit| left.rows.len().saturating_mul(index.max_dup.max(1)) > limit);
    let estimated_output = if estimate_needed {
        let limit = max_output_rows.expect("estimate_needed 蕴含存在上限");
        let preflight_started = Instant::now();
        let estimate = index.estimate(left, &spec.left_keys, spec.join_type, &mut left_key);
        timings.preflight = preflight_started.elapsed();
        timings.estimate = Some(estimate);
        if estimate.output_rows > limit {
            timings.total = started.elapsed();
            return Err(JoinLimitExceeded { estimate, timings });
        }
        Some(estimate.output_rows)
    } else {
        None
    };

    let rp_valid: Vec<usize> = spec
        .right_pick
        .iter()
        .copied()
        .filter(|&c| c < right.col_count())
        .collect();
    let lp_valid: Vec<usize> = spec.resolved_left_pick(left.col_count());
    let mut headers: Vec<String> = lp_valid
        .iter()
        .map(|&col| left.headers[col].clone())
        .collect();
    for &col in &rp_valid {
        headers.push(right.headers[col].clone());
    }
    let mut joined_rows = Vec::new();
    // 有精确预估值就用它;否则 Left 在「不展开」或「上界已证明不超过上限」
    // 两种情形下每行至少产出一行,按 A 行数预留即可(上界保证不会更大)。
    let reserve_rows = estimated_output.or_else(|| {
        let one_row_per_left =
            spec.join_type == JoinType::Left && (!spec.expand_dup || max_output_rows.is_some());
        one_row_per_left.then_some(left.rows.len())
    });
    if let Some(rows) = reserve_rows.filter(|&rows| rows < usize::MAX) {
        joined_rows.reserve(rows);
    }

    let mut left_matched = 0usize;
    let mut matched_rows = 0usize;
    let mut right_used: Vec<bool> = vec![false; right.rows.len()];
    let measure = collect_timings;
    let mut probe_time = Duration::ZERO;
    let mut materialize_time = Duration::ZERO;

    // ── 左表驱动(left / inner) ──
    for (left_index, row) in left.rows.iter().enumerate() {
        let probe_started = measure.then(Instant::now);
        let hit = if write_key(row, &spec.left_keys, spec.key_mode, &mut left_key) {
            index.lookup(&left_key)
        } else {
            None
        };
        if let Some(probe_started) = probe_started {
            probe_time += probe_started.elapsed();
        }

        match hit {
            Some(matches) => {
                left_matched += 1;
                matches.for_each_selected(spec.expand_dup, |right_index| {
                    right_used[right_index] = true;
                    let reference_started = measure.then(Instant::now);
                    joined_rows.push(JoinedRow {
                        left_row: left_index as u32,
                        right_row: right_index as u32,
                    });
                    matched_rows += 1;
                    if let Some(reference_started) = reference_started {
                        materialize_time += reference_started.elapsed();
                    }
                });
            }
            None if spec.join_type == JoinType::Left => {
                let reference_started = measure.then(Instant::now);
                joined_rows.push(JoinedRow {
                    left_row: left_index as u32,
                    right_row: UNMATCHED_RIGHT_ROW,
                });
                if let Some(reference_started) = reference_started {
                    materialize_time += reference_started.elapsed();
                }
            }
            None => {}
        }
    }

    timings.probe = probe_time;
    timings.materialize = materialize_time;
    timings.total = started.elapsed();

    let right_matched_rows = right_used.iter().filter(|&&used| used).count();
    let out_rows = joined_rows.len();
    Ok((
        JoinResult {
            table: JoinedTable {
                headers,
                rows: joined_rows,
                left_pick: lp_valid,
                right_pick: rp_valid,
            },
            left_total: left.rows.len(),
            left_matched,
            right_total: right.rows.len(),
            right_matched_rows,
            out_rows,
            matched_rows,
        },
        timings,
    ))
}

/// 带输出上限的 Join。预估与真正 Join 共享一次构建的 B 索引。
pub fn join_with_limit(
    left: &Table,
    right: &Table,
    spec: &JoinSpec,
    max_output_rows: Option<usize>,
) -> Result<JoinResult, JoinLimitExceeded> {
    run_join(left, right, spec, max_output_rows, false).map(|(result, _)| result)
}

/// 带分阶段计时的 Join，供 benchmark 使用；不设置输出上限。
pub fn join_with_metrics(
    left: &Table,
    right: &Table,
    spec: &JoinSpec,
) -> (JoinResult, JoinTimings) {
    run_join(left, right, spec, None, true).expect("未设置输出上限时 Join 不会超限")
}

/// 带输出上限和分阶段计时的 Join，供 benchmark/诊断使用。
pub fn join_with_limit_metrics(
    left: &Table,
    right: &Table,
    spec: &JoinSpec,
    max_output_rows: Option<usize>,
) -> Result<(JoinResult, JoinTimings), JoinLimitExceeded> {
    run_join(left, right, spec, max_output_rows, true)
}

pub fn join(left: &Table, right: &Table, spec: &JoinSpec) -> JoinResult {
    run_join(left, right, spec, None, false)
        .expect("未设置输出上限时 Join 不会超限")
        .0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    fn tbl(headers: &[&str], rows: &[&[&str]]) -> Table {
        let mut t = Table::new(headers.iter().map(|s| s.to_string()).collect());
        for r in rows {
            t.push_row(r.iter().map(|s| CellValue::Text(s.to_string())).collect());
        }
        t
    }

    fn spec(jt: JoinType, lk: usize, rk: usize, pick: usize, mode: KeyMode) -> JoinSpec {
        JoinSpec {
            join_type: jt,
            left_keys: vec![lk],
            right_keys: vec![rk],
            left_pick: None,
            right_pick: vec![pick],
            key_mode: mode,
            expand_dup: true,
        }
    }

    fn joined_cell<'a>(
        table: &JoinedTable,
        left: &'a Table,
        right: &'a Table,
        row: usize,
        column: usize,
    ) -> Option<&'a CellValue> {
        table.cell(left, right, row, column)
    }

    /// 逐行取出命中标志,替代已删除的 `JoinResult::row_hit` 平行数组。
    fn hits(result: &JoinResult) -> Vec<bool> {
        (0..result.table.row_count())
            .map(|row| result.table.row_hit(row))
            .collect()
    }

    #[test]
    fn selected_left_columns_preserve_order_values_and_row_matches() {
        let left = tbl(
            &["姓名", "案号", "金额", "备注"],
            &[
                &["甲", "台88（2025）执388号之十二", "100", "保留"],
                &["乙", "台88（2025）执388号之二", "200", "保留"],
                &["丙", "未命中", "300", "保留"],
            ],
        );
        let right = tbl(&["案号", "部门"], &[&["台88（2025）执388号", "执行部门"]]);
        let mut join_spec = spec(
            JoinType::Left,
            1,
            0,
            1,
            KeyMode {
                case_suffix: true,
                ..KeyMode::EXACT
            },
        );
        // 非连续列且改变输出顺序;匹配键本身不输出。
        join_spec.left_pick = Some(vec![2, 0, 99]);
        let result = join(&left, &right, &join_spec);
        assert_eq!(result.table.headers, ["金额", "姓名", "部门"]);
        assert_eq!(result.table.left_pick, [2, 0]);
        assert_eq!(result.left_matched, 2);
        assert_eq!(hits(&result), [true, true, false]);
        let expected = tbl(
            &["金额", "姓名", "部门"],
            &[
                &["100", "甲", "执行部门"],
                &["200", "乙", "执行部门"],
                &["300", "丙", ""],
            ],
        );
        let mut expected_rows = expected.rows;
        expected_rows[2][2] = CellValue::Empty;
        assert_eq!(result.table.materialize(&left, &right).rows, expected_rows);
        for (row, cells) in expected_rows.iter().enumerate() {
            for (column, expected) in cells.iter().enumerate() {
                assert_eq!(
                    result.table.cell(&left, &right, row, column),
                    Some(expected)
                );
            }
        }
        assert_eq!(result.table.cell(&left, &right, 0, 3), None);
    }

    #[test]
    fn no_left_output_columns_still_match_and_expand_duplicates() {
        let left = tbl(&["id"], &[&["1"], &["2"]]);
        let right = tbl(&["id", "部门"], &[&["1", "甲"], &["1", "乙"]]);
        let mut join_spec = spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT);
        join_spec.left_pick = Some(vec![]);
        let result = join(&left, &right, &join_spec);
        assert_eq!(result.table.headers, ["部门"]);
        assert!(result.table.left_pick.is_empty());
        assert_eq!(result.table.row_count(), 3);
        assert_eq!(hits(&result), [true, true, false]);
        assert_eq!(
            result.table.materialize(&left, &right).rows,
            vec![
                vec![CellValue::from("甲")],
                vec![CellValue::from("乙")],
                vec![CellValue::Empty],
            ]
        );
    }

    #[test]
    fn out_of_range_left_columns_are_dropped_not_kept_as_empty() {
        // 全部越界 = 左表一列都不输出;这里顺带钉住越界项不会变成一列空列。
        let left = tbl(&["id", "姓名"], &[&["1", "甲"], &["2", "乙"]]);
        let right = tbl(&["id", "部门"], &[&["1", "工程部"], &["2", "产品部"]]);
        let mut join_spec = spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT);
        join_spec.left_pick = Some(vec![99, 100]);
        let result = join(&left, &right, &join_spec);
        assert_eq!(result.table.headers, ["部门"]);
        assert!(result.table.left_pick.is_empty());
        assert_eq!(result.table.col_count(), 1);
        assert_eq!(result.table.row_count(), 2);
        assert_eq!(result.left_matched, 2);
        assert_eq!(
            result.table.materialize(&left, &right).rows,
            vec![
                vec![CellValue::from("工程部")],
                vec![CellValue::from("产品部")],
            ]
        );
        // 输出只剩 B 列:A 侧任何列号都读不到值
        assert_eq!(result.table.cell(&left, &right, 0, 1), None);
        // A 侧全越界,但 B 侧还有输出列 → 整体仍有输出
        assert!(join_spec.has_output_columns(left.col_count(), right.col_count()));
        let mut no_right = join_spec.clone();
        no_right.right_pick.clear();
        assert!(!no_right.has_output_columns(left.col_count(), right.col_count()));
        assert!(no_right.resolved_left_pick(left.col_count()).is_empty());
    }

    #[test]
    fn has_output_columns_matches_resolved_left_pick() {
        let spec_with = |left_pick: Option<Vec<usize>>, right_pick: Vec<usize>| {
            let mut join_spec = spec(JoinType::Left, 0, 0, 0, KeyMode::EXACT);
            join_spec.left_pick = left_pick;
            join_spec.right_pick = right_pick;
            join_spec
        };
        // None = 默认全输出:A 有列就算有输出
        assert!(spec_with(None, vec![]).has_output_columns(2, 2));
        assert!(!spec_with(None, vec![]).has_output_columns(0, 0));
        // Some 全越界 + B 无可输出列 = 没有任何输出(UI 与 worker 必须同时判否)
        assert!(!spec_with(Some(vec![7]), vec![]).has_output_columns(2, 2));
        assert!(!spec_with(Some(vec![7]), vec![9]).has_output_columns(2, 3));
        // 任一侧还有有效列就算有输出
        assert!(spec_with(Some(vec![]), vec![1]).has_output_columns(2, 3));
        assert!(spec_with(Some(vec![1]), vec![]).has_output_columns(2, 3));
        // 与真正参与 Join 的解析结果一致
        for (left_pick, left_cols) in [
            (None, 3),
            (Some(vec![]), 3),
            (Some(vec![2, 0, 99]), 3),
            (Some(vec![99]), 3),
        ] {
            let join_spec = spec_with(left_pick, vec![]);
            assert_eq!(
                join_spec.has_output_columns(left_cols, 0),
                !resolve_left_pick(join_spec.left_pick.as_deref(), left_cols).is_empty()
            );
        }
    }

    #[test]
    fn has_key_output_requires_a_key_column_in_output() {
        // 键列 lk=0 / rk=0;left_pick 与 right_pick 逐用例覆盖。
        let spec_with = |left_pick: Option<Vec<usize>>, right_pick: Vec<usize>| {
            let mut join_spec = spec(JoinType::Left, 0, 0, 0, KeyMode::EXACT);
            join_spec.left_pick = left_pick;
            join_spec.right_pick = right_pick;
            join_spec
        };
        // A 默认全选(None)含键列 → 有匹配列输出
        assert!(spec_with(None, vec![1]).has_key_output(2, 2));
        // A 显式空选 + B 只输出非键列 → 没有任何匹配列输出
        assert!(!spec_with(Some(vec![]), vec![1]).has_key_output(2, 2));
        // B 键列补进 right_pick → 满足
        assert!(spec_with(Some(vec![]), vec![0, 1]).has_key_output(2, 2));
        // A、B 输出列都不含各自键列 → 不满足
        assert!(!spec_with(Some(vec![1]), vec![1]).has_key_output(2, 2));
        // A 显式选了键列即满足,B 侧可以为空
        assert!(spec_with(Some(vec![0]), vec![]).has_key_output(2, 2));
        // 键列下标越界(A 表只有 0 列)→ 视为未输出
        assert!(!spec_with(Some(vec![0]), vec![]).has_key_output(0, 2));
        // B 键列越界同理
        assert!(!spec_with(Some(vec![]), vec![0]).has_key_output(2, 0));
        // 方法与自由函数判定一致
        let join_spec = spec_with(Some(vec![]), vec![1]);
        assert_eq!(
            join_spec.has_key_output(2, 2),
            has_key_output(&[0], &[0], Some(&[]), &[1], 2, 2)
        );
        let join_spec = spec_with(Some(vec![0]), vec![]);
        assert_eq!(
            join_spec.has_key_output(2, 2),
            has_key_output(&[0], &[0], Some(&[0]), &[], 2, 2)
        );
    }

    #[test]
    fn joined_row_stays_compact() {
        // 结果行数可达 500 万,行引用按 8B/行 存放;退回 Option<usize> 会变成 24B/行。
        assert_eq!(std::mem::size_of::<JoinedRow>(), 8);
    }

    #[test]
    fn left_join_vlookup() {
        let a = tbl(
            &["id", "name"],
            &[&["1", "alice"], &["2", "bob"], &["3", "carol"]],
        );
        let b = tbl(&["id", "dept"], &[&["1", "eng"], &["3", "ops"]]);
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(r.table.headers, vec!["id", "name", "dept"]);
        assert_eq!(r.table.row_count(), 3);
        assert_eq!(
            r.table.rows,
            vec![
                JoinedRow {
                    left_row: 0,
                    right_row: 0,
                },
                JoinedRow {
                    left_row: 1,
                    right_row: UNMATCHED_RIGHT_ROW,
                },
                JoinedRow {
                    left_row: 2,
                    right_row: 1,
                },
            ]
        );
        assert_eq!(joined_cell(&r.table, &a, &b, 0, 2), Some(&CellValue::Text("eng".into())));
        assert_eq!(joined_cell(&r.table, &a, &b, 1, 2), Some(&CellValue::Empty)); // bob 未匹配
        assert_eq!(joined_cell(&r.table, &a, &b, 2, 2), Some(&CellValue::Text("ops".into())));
        assert_eq!(r.left_matched, 2);
    }

    #[test]
    fn inner_join_drops_unmatched() {
        let a = tbl(&["id"], &[&["1"], &["2"], &["3"]]);
        let b = tbl(&["id", "v"], &[&["2", "x"], &["3", "y"]]);
        let r = join(&a, &b, &spec(JoinType::Inner, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(r.table.row_count(), 2);
        assert_eq!(joined_cell(&r.table, &a, &b, 0, 1), Some(&CellValue::Text("x".into())));
        assert_eq!(joined_cell(&r.table, &a, &b, 1, 1), Some(&CellValue::Text("y".into())));
        // inner:未命中行被丢弃,输出全为命中行
        assert_eq!(hits(&r), vec![true, true]);
    }

    #[test]
    fn row_hit_marks_left_unmatched() {
        // 左连接:匹配上的行 hit=true;未匹配 A 行(补空)hit=false
        let a = tbl(&["id"], &[&["1"], &["2"], &["3"]]);
        let b = tbl(&["id", "v"], &[&["1", "x"], &["3", "y"]]);
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(r.table.row_count(), 3);
        assert_eq!(hits(&r), vec![true, false, true]);
    }

    #[test]
    fn duplicate_right_key_fanout() {
        // 右表同 key 两行 → left 行复制成两行(vlookup 对重复键取第一条,这里取全部)
        let a = tbl(&["id"], &[&["1"]]);
        let b = tbl(&["id", "v"], &[&["1", "a"], &["1", "b"]]);
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(r.table.row_count(), 2);
        assert_eq!(joined_cell(&r.table, &a, &b, 0, 1), Some(&CellValue::Text("a".into())));
        assert_eq!(joined_cell(&r.table, &a, &b, 1, 1), Some(&CellValue::Text("b".into())));
        // 展开的两行都算命中
        assert_eq!(hits(&r), vec![true, true]);
        assert_eq!(r.left_matched, 1);
        assert_eq!(r.right_matched_rows, 2);
    }

    #[test]
    fn no_expand_dup_takes_first_only() {
        // expand_dup=false:同 key 只取 B 第一条(经典 VLOOKUP),不展开
        let a = tbl(&["id"], &[&["1"]]);
        let b = tbl(&["id", "v"], &[&["1", "a"], &["1", "b"]]);
        let mut sp = spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT);
        sp.expand_dup = false;
        let r = join(&a, &b, &sp);
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(joined_cell(&r.table, &a, &b, 0, 1), Some(&CellValue::Text("a".into())));
        assert_eq!(hits(&r), vec![true]);
        assert_eq!(r.right_matched_rows, 1); // 只算实际用到的一条 B
    }

    #[test]
    fn estimate_join_rows_counts_dup() {
        // 3 个 A 键,每个在 B 命中 2 条 → 预估 6;B 最大单键重复 2,去重后 3
        let a = tbl(&["id"], &[&["1"], &["2"], &["3"]]);
        let b = tbl(
            &["id", "v"],
            &[
                &["1", "a"],
                &["1", "b"],
                &["2", "c"],
                &["2", "d"],
                &["3", "e"],
                &["3", "f"],
            ],
        );
        let estimate = estimate_join_rows(&a, &b, &[0], &[0], KeyMode::EXACT, JoinType::Left, true);
        assert_eq!(estimate.output_rows, 6);
        assert_eq!(estimate.max_dup, 2);
        assert_eq!(estimate.distinct_keys, 3);
    }

    #[test]
    fn estimate_matches_left_and_inner_output_contract() {
        // A=[1,9],B=[1,1]:Left 展开输出 2 个命中展开行 + 1 个未命中保留行;
        // Inner 只输出 2 个命中行。关闭展开时每个命中键只占 1 行。
        let a = tbl(&["id"], &[&["1"], &["9"]]);
        let b = tbl(&["id", "v"], &[&["1", "a"], &["1", "b"]]);

        let left_expand =
            estimate_join_rows(&a, &b, &[0], &[0], KeyMode::EXACT, JoinType::Left, true);
        assert_eq!(left_expand.output_rows, 3);

        let inner_expand =
            estimate_join_rows(&a, &b, &[0], &[0], KeyMode::EXACT, JoinType::Inner, true);
        assert_eq!(inner_expand.output_rows, 2);

        let left_first =
            estimate_join_rows(&a, &b, &[0], &[0], KeyMode::EXACT, JoinType::Left, false);
        assert_eq!(left_first.output_rows, 2);
        assert_eq!(left_first.max_dup, 2);
        assert_eq!(left_first.distinct_keys, 1);

        let inner_first =
            estimate_join_rows(&a, &b, &[0], &[0], KeyMode::EXACT, JoinType::Inner, false);
        assert_eq!(inner_first.output_rows, 1);
    }

    #[test]
    fn estimate_counts_unmatched_left_rows() {
        let a = tbl(&["id"], &[&["9"]]);
        let b = tbl(&["id", "v"], &[&["1", "a"]]);
        let left = estimate_join_rows(&a, &b, &[0], &[0], KeyMode::EXACT, JoinType::Left, true);
        assert_eq!(left.output_rows, 1);

        let inner = estimate_join_rows(&a, &b, &[0], &[0], KeyMode::EXACT, JoinType::Inner, true);
        assert_eq!(inner.output_rows, 0);
    }

    #[test]
    fn estimate_saturates_and_limit_is_strict() {
        assert_eq!(add_output_rows(usize::MAX - 1, 2), usize::MAX);

        let a = tbl(&["id"], &[&["1"], &["9"]]);
        let b = tbl(&["id", "v"], &[&["1", "a"], &["1", "b"]]);
        let mut sp = spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT);
        // 预估正好等于上限时允许；超过上限才拒绝。
        assert!(join_with_limit(&a, &b, &sp, Some(3)).is_ok());
        assert!(join_with_limit(&a, &b, &sp, Some(2)).is_err());
        sp.expand_dup = false;
        assert!(join_with_limit(&a, &b, &sp, Some(2)).is_ok());
    }

    #[test]
    fn upper_bound_within_limit_skips_estimate_with_same_output() {
        // A 两行、B 三行同键:上界 = 2 × 3 = 6,Left 展开实际输出 3 + 1 = 4 行。
        let a = tbl(&["id"], &[&["1"], &["9"]]);
        let b = tbl(&["id", "v"], &[&["1", "a"], &["1", "b"], &["1", "c"]]);
        let sp = spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT);

        // 上限 6 ≥ 上界 → 判定不可能超限,跳过 A 侧预估扫描直接连接。
        let (skipped, skipped_timings) = join_with_limit_metrics(&a, &b, &sp, Some(6)).unwrap();
        assert!(
            skipped_timings.estimate.is_none(),
            "上界不超过上限时不应再做整张 A 表的预估扫描"
        );
        assert_eq!(skipped.out_rows, 4);
        assert_eq!(skipped.matched_rows, 3);
        assert_eq!(skipped.left_matched, 1);

        // 上限 5 < 上界 → 仍走预估路径,输出必须逐行一致。
        let (estimated, estimated_timings) = join_with_limit_metrics(&a, &b, &sp, Some(5)).unwrap();
        assert_eq!(estimated_timings.estimate.map(|e| e.output_rows), Some(4));
        assert_eq!(estimated.table.rows, skipped.table.rows);
        assert_eq!(estimated.matched_rows, skipped.matched_rows);
    }

    #[test]
    fn estimate_unmatched_is_not_counted_for_inner() {
        // 保留一个单独的 Inner 回归断言，避免以后只针对 Left 修复时丢掉口径。
        let a2 = tbl(&["id"], &[&["9"]]);
        let b2 = tbl(&["id", "v"], &[&["1", "a"]]);
        let estimate =
            estimate_join_rows(&a2, &b2, &[0], &[0], KeyMode::EXACT, JoinType::Inner, true);
        assert_eq!(estimate.output_rows, 0);
    }

    #[test]
    fn estimate_equals_materialized_output_for_both_join_types() {
        let a = tbl(&["id"], &[&["1"], &["9"], &["1"]]);
        let b = tbl(&["id", "v"], &[&["1", "a"], &["1", "b"]]);
        for join_type in JoinType::all() {
            for expand_dup in [false, true] {
                let estimate =
                    estimate_join_rows(&a, &b, &[0], &[0], KeyMode::EXACT, join_type, expand_dup);
                let mut join_spec = spec(join_type, 0, 0, 1, KeyMode::EXACT);
                join_spec.expand_dup = expand_dup;
                let result = join_with_limit(&a, &b, &join_spec, Some(usize::MAX)).unwrap();
                assert_eq!(estimate.output_rows, result.out_rows);
            }
        }
    }

    #[test]
    fn composite_key() {
        let a = tbl(&["k1", "k2", "v"], &[&["a", "1", "l1"]]);
        let b = tbl(&["k1", "k2", "w"], &[&["a", "1", "r1"], &["a", "2", "r2"]]);
        let r = join(
            &a,
            &b,
            &JoinSpec {
                join_type: JoinType::Left,
                left_keys: vec![0, 1],
                right_keys: vec![0, 1],
                left_pick: None,
                right_pick: vec![2],
                key_mode: KeyMode::EXACT,
                expand_dup: true,
            },
        );
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(r.table.headers, vec!["k1", "k2", "v", "w"]);
        assert_eq!(joined_cell(&r.table, &a, &b, 0, 3), Some(&CellValue::Text("r1".into())));
    }

    #[test]
    fn normalize_key_matches_number_text() {
        let a = tbl(&["id"], &[&["123"]]); // 文本 123
        let mut tb = Table::new(vec!["id".into(), "v".into()]);
        tb.push_row(vec![
            CellValue::Number(123.0),
            CellValue::Text("num".into()),
        ]);
        let r = join(&a, &tb, &spec(JoinType::Left, 0, 0, 1, KeyMode::NORMALIZE));
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(joined_cell(&r.table, &a, &tb, 0, 1), Some(&CellValue::Text("num".into())));

        // Exact 下数字 123 ≠ 文本 "123"
        let r2 = join(&a, &tb, &spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(joined_cell(&r2.table, &a, &tb, 0, 1), Some(&CellValue::Empty));
    }

    #[test]
    fn normalize_does_not_make_leading_zero_number_equal() {
        let a = tbl(&["id"], &[&["001"]]);
        let mut b = Table::new(vec!["id".into(), "v".into()]);
        b.push_row(vec![
            CellValue::Number(1.0),
            CellValue::Text("number".into()),
        ]);
        let result = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::NORMALIZE));
        assert_eq!(joined_cell(&result.table, &a, &b, 0, 1), Some(&CellValue::Empty));
    }

    #[test]
    fn empty_key_not_matched() {
        // 左表空键行 → left 保留但右侧空
        let a = tbl(&["id"], &[&[""], &["1"]]);
        let b = tbl(&["id", "v"], &[&["1", "x"]]);
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(r.table.row_count(), 2);
        assert_eq!(joined_cell(&r.table, &a, &b, 0, 1), Some(&CellValue::Empty));
        assert_eq!(joined_cell(&r.table, &a, &b, 1, 1), Some(&CellValue::Text("x".into())));
    }

    #[test]
    fn multiple_pick_columns() {
        let a = tbl(&["id"], &[&["1"]]);
        let b = tbl(&["id", "x", "y"], &[&["1", "10", "20"]]);
        let r = join(
            &a,
            &b,
            &JoinSpec {
                join_type: JoinType::Left,
                left_keys: vec![0],
                right_keys: vec![0],
                left_pick: None,
                right_pick: vec![1, 2],
                key_mode: KeyMode::EXACT,
                expand_dup: true,
            },
        );
        assert_eq!(r.table.headers, vec!["id", "x", "y"]);
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(joined_cell(&r.table, &a, &b, 0, 2), Some(&CellValue::Text("20".into())));
    }

    #[test]
    fn case_suffix_supported_forms_and_boundaries() {
        let base = "台88（2025）执388号";
        for suffix in [
            "之一",
            "之十二",
            "之二十三",
            "之一百零二",
            "之两千〇一",
            "之一万",
            "（之十二）",
            "(之十二)",
        ] {
            let original = format!("{base}{suffix}");
            assert_eq!(strip_case_suffix(&original), base, "{original}");
        }
        for suffix in [
            "",
            "之",
            "之12",
            "之壹",
            "之十二附注",
            "（之十二)",
            "(之十二）",
            "（之十二",
            "之十二）",
            "之一（之二）",
        ] {
            let original = format!("{base}{suffix}");
            assert_eq!(strip_case_suffix(&original), original, "{original}");
        }
        assert_eq!(strip_case_suffix("普通文本之十二"), "普通文本之十二");
    }

    #[test]
    fn case_suffix_join_preserves_left_rows_and_original_values() {
        let a = tbl(
            &["文书号"],
            &[
                &["台88（2025）民初232号之一"],
                &["台88（2025）民初232号之十二"],
            ],
        );
        let b = tbl(
            &["案号", "承办部门"],
            &[&["台88（2025）民初232号", "某部门"]],
        );
        let mode = KeyMode {
            case_suffix: true,
            ..KeyMode::EXACT
        };
        for expand_dup in [false, true] {
            let mut sp = spec(JoinType::Left, 0, 0, 1, mode);
            sp.expand_dup = expand_dup;
            let result = join_with_limit(&a, &b, &sp, Some(2)).unwrap();
            assert_eq!(hits(&result), vec![true, true]);
            for row in 0..2 {
                assert_eq!(
                    joined_cell(&result.table, &a, &b, row, 0),
                    Some(&a.rows[row][0])
                );
                assert_eq!(
                    joined_cell(&result.table, &a, &b, row, 1),
                    Some(&b.rows[0][1])
                );
            }
        }
        let disabled = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::default()));
        assert_eq!(hits(&disabled), vec![false, false]);
    }

    #[test]
    fn case_suffix_cleans_both_sides_and_respects_limits() {
        let a = tbl(&["案号"], &[&[" 台88（2025）执388号之十二 "]]);
        let b = tbl(
            &["案号", "承办部门"],
            &[
                &["台88(2025)执388号(之二十三)", "甲"],
                &["台88(2025)执388号", "乙"],
            ],
        );
        let mode = KeyMode {
            case_suffix: true,
            ..KeyMode::default()
        };
        let mut sp = spec(JoinType::Left, 0, 0, 1, mode);
        assert_eq!(
            estimate_join_rows(&a, &b, &[0], &[0], mode, JoinType::Left, true).output_rows,
            2
        );
        assert!(join_with_limit(&a, &b, &sp, Some(1)).is_err());
        assert_eq!(
            hits(&join_with_limit(&a, &b, &sp, Some(2)).unwrap()),
            vec![true, true]
        );
        sp.expand_dup = false;
        assert_eq!(
            hits(&join_with_limit(&a, &b, &sp, Some(1)).unwrap()),
            vec![true]
        );
        sp.key_mode.number_text = false;
        assert_eq!(hits(&join(&a, &b, &sp)), vec![false]);
    }

    #[test]
    fn bracket_fold_matches_chinese_english() {
        // 开启括号归一化:中文（）与英文 () 互认
        let a = tbl(&["name"], &[&["苹果（红）"]]);
        let b = tbl(&["name", "v"], &[&["苹果(红)", "x"]]);
        let m = KeyMode {
            number_text: false,
            brackets: true,
            case_suffix: false,
        };
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m));
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(joined_cell(&r.table, &a, &b, 0, 1), Some(&CellValue::Text("x".into())));

        // 关闭括号归一化则不匹配
        let m2 = KeyMode {
            number_text: false,
            brackets: false,
            case_suffix: false,
        };
        let r2 = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m2));
        assert_eq!(r2.table.row_count(), 1);
        assert_eq!(joined_cell(&r2.table, &a, &b, 0, 1), Some(&CellValue::Empty));
    }

    #[test]
    fn bracket_fold_covers_all_pairs() {
        // 【】→[]、｛｝→{} 等成对折叠
        let a = tbl(&["k"], &[&["型号【A】｛B｝〔C〕"]]);
        let b = tbl(&["k", "v"], &[&["型号[A]{B}(C)", "hit"]]);
        let m = KeyMode {
            number_text: false,
            brackets: true,
            case_suffix: false,
        };
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m));
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(joined_cell(&r.table, &a, &b, 0, 1), Some(&CellValue::Text("hit".into())));
    }

    #[test]
    fn bracket_fold_independent_of_number_text() {
        // 括号归一化与数字/文本互认是独立开关:开括号时数字仍精确
        let a = tbl(&["id"], &[&["1（a）"]]);
        let b = tbl(&["id", "v"], &[&["1(a)", "x"]]);
        let m = KeyMode {
            number_text: false,
            brackets: true,
            case_suffix: false,
        };
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m));
        assert_eq!(joined_cell(&r.table, &a, &b, 0, 1), Some(&CellValue::Text("x".into())));
    }

    #[test]
    fn trim_and_empty_cell_semantics_remain_distinct() {
        let mut a = Table::new(vec!["id".into()]);
        a.push_row(vec![CellValue::Empty]);
        a.push_row(vec![CellValue::Text(String::new())]);
        a.push_row(vec![CellValue::Text("   ".into())]);
        let mut b = Table::new(vec!["id".into(), "v".into()]);
        b.push_row(vec![
            CellValue::Text(String::new()),
            CellValue::Text("text".into()),
        ]);

        let exact = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(hits(&exact), vec![false, true, false]);

        let normalized = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::NORMALIZE));
        // Text("") 与纯空白在 trim 后相同，但仍不与 Empty 相同。
        assert_eq!(hits(&normalized), vec![false, true, true]);
    }

    #[test]
    fn reused_key_buffer_clears_after_partial_write_and_empty_key() {
        let row = vec![CellValue::Text("first".into())];
        let mut key = String::from("stale-key");

        // 第一列已经写入后，第二列越界；失败路径不能留下部分 key。
        assert!(!write_key(&row, &[0, 1], KeyMode::EXACT, &mut key));
        assert!(key.is_empty());

        // 单列 Empty 和全 Empty 复合键也必须清空上一次内容。
        assert!(!write_key(
            &[CellValue::Empty],
            &[0],
            KeyMode::EXACT,
            &mut key
        ));
        assert!(key.is_empty());
        assert!(!write_key(
            &[CellValue::Empty, CellValue::Empty],
            &[0, 1],
            KeyMode::EXACT,
            &mut key
        ));
        assert!(key.is_empty());
    }

    #[test]
    fn reused_key_buffer_handles_long_short_and_bracketed_keys() {
        let mode = KeyMode {
            number_text: true,
            brackets: true,
            case_suffix: false,
        };
        let mut key = String::new();
        let long = vec![CellValue::Text("  很长的键值（A）以及尾部  ".into())];
        assert!(write_key(&long, &[0], mode, &mut key));
        assert_eq!(key, "V:很长的键值(A)以及尾部");

        let short = vec![CellValue::Text("x".into())];
        assert!(write_key(&short, &[0], mode, &mut key));
        assert_eq!(key, "V:x");
    }

    #[test]
    fn bracket_fold_borrows_when_no_replacement_is_needed() {
        assert!(matches!(fold_brackets("plain"), Cow::Borrowed("plain")));
        assert_eq!(fold_brackets("型号（A）").as_ref(), "型号(A)");
    }

    #[test]
    fn composite_separator_collision_is_unchanged_and_documented() {
        // 历史复合键用 U+0001 分隔且不转义：
        // ["x", "\u{1}S:d"] 与 ["x\u{1}S:", "d"] 会编码成同一 key。
        let a = tbl(&["k1", "k2"], &[&["x", "\u{1}S:d"]]);
        let b = tbl(&["k1", "k2", "v"], &[&["x\u{1}S:", "d", "hit"]]);
        let result = join(
            &a,
            &b,
            &JoinSpec {
                join_type: JoinType::Left,
                left_keys: vec![0, 1],
                right_keys: vec![0, 1],
                left_pick: None,
                right_pick: vec![2],
                key_mode: KeyMode::EXACT,
                expand_dup: true,
            },
        );
        assert_eq!(
            joined_cell(&result.table, &a, &b, 0, 2),
            Some(&CellValue::Text("hit".into()))
        );
    }
}
