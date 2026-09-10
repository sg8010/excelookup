//! Join 引擎:实现 VLOOKUP(left join)与 inner join
//!
//! 左表按「原列」输出;右表只输出「取值列」,避免键列重复。
//! 键支持多列复合;归一化可配置:数字/文本互认+trim、中文/英文括号互认。

use std::borrow::Cow;
use std::collections::{HashMap, hash_map::Entry};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use crate::model::{CellValue, Table};

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

/// 键归一化策略(两个独立开关,可任意组合)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyMode {
    /// 宽松匹配:数字 1 与文本 "1" 视为相同,并 trim 首尾空白
    pub number_text: bool,
    /// 括号归一化:中文括号(（）【】｛｝)与英文括号([]{})互认
    pub brackets: bool,
}

impl KeyMode {
    /// 全部关闭:数字 1 与文本 "1" 视为不同,括号不归一化
    pub const EXACT: Self = Self {
        number_text: false,
        brackets: false,
    };
    /// 仅数字/文本互认 + trim(不带括号归一化)
    pub const NORMALIZE: Self = Self {
        number_text: true,
        brackets: false,
    };
}

impl Default for KeyMode {
    fn default() -> Self {
        // 默认:两开关全开(数字/文本互认 + 括号归一化)
        Self {
            number_text: true,
            brackets: true,
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
    /// 右表取值列下标(结果中仅这些列来自右表)
    pub right_pick: Vec<usize>,
    pub key_mode: KeyMode,
    /// 右表同键多行时是否全部展开。true=逐行展开(VLOOKUP 的重复键也取全);
    /// false=只取第一条(经典 VLOOKUP 语义),避免结果行数爆炸。
    pub expand_dup: bool,
}

/// join 结果
#[derive(Debug, Clone)]
pub struct JoinResult {
    /// 输出表:左表全部列 + 右表取值列
    pub table: Table,
    pub left_total: usize,
    pub left_matched: usize,
    pub right_total: usize,
    /// 右表行中被匹配过的行数
    pub right_matched_rows: usize,
    pub out_rows: usize,
    /// 输出表每行是否命中(与 table.rows 对齐;左连接未匹配的 A 行 = false,内连接全 true)
    pub row_hit: Vec<bool>,
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
/// `preflight` 是带上限连接在真正物化前做的 A 侧预估扫描；`probe` 与
/// `materialize` 是真正输出扫描中的两个部分。普通 `join` 不采集这些计时。
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
    rows: HashMap<String, RowMatches>,
    max_dup: usize,
    expand_dup: bool,
    mode: KeyMode,
}

impl JoinIndex {
    fn build(right: &Table, rk: &[usize], mode: KeyMode, expand_dup: bool) -> Self {
        // 不按 B 总行数无条件 reserve：高重复数据的 distinct key 数可能远小于
        // B 行数，HashMap 按需增长能避免先分配一大块空桶。
        let mut rows: HashMap<String, RowMatches> = HashMap::new();
        let mut max_dup = 0usize;

        for (i, row) in right.rows.iter().enumerate() {
            let Some(key) = make_key(row, rk, mode) else {
                continue;
            };
            match rows.entry(key.into_owned()) {
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
fn fold_brackets(s: &str) -> Cow<'_, str> {
    if !s.chars().any(is_foldable_bracket) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(
        s.chars()
            .map(|c| match c {
                '（' => '(',
                '）' => ')',
                '【' => '[',
                '】' => ']',
                '｛' => '{',
                '｝' => '}',
                '〔' => '(',
                '〕' => ')',
                other => other,
            })
            .collect(),
    )
}

fn is_foldable_bracket(c: char) -> bool {
    matches!(c, '（' | '）' | '【' | '】' | '｛' | '｝' | '〔' | '〕')
}

/// 文本正文归一化：trim 始终借用切片；括号没有发生替换时也借用。
fn normalized_text(s: &str, mode: KeyMode) -> Cow<'_, str> {
    let trimmed = if mode.number_text { s.trim() } else { s };
    if mode.brackets {
        fold_brackets(trimmed)
    } else {
        Cow::Borrowed(trimmed)
    }
}

/// 向目标 key 追加一个带类型前缀的片段，返回该片段是否非空。
///
/// 单列路径会直接调用这个函数，不经过 `Vec<String> + join`；前缀使规范
/// key 必须拥有自己的字符串，这一点不能被 Cow 借用消除。
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
            let body = normalized_text(s, mode);
            let prefix = if mode.number_text { "V:" } else { "S:" };
            out.reserve(prefix.len() + body.len());
            out.push_str(prefix);
            out.push_str(body.as_ref());
            true
        }
    }
}

/// 单列 key 快速路径。非空文本/数字仍需为类型前缀创建一个规范 key，
/// 但不再产生复合键用的中间 Vec 与 join 字符串。
fn make_single_key<'a>(row: &'a [CellValue], col: usize, mode: KeyMode) -> Option<Cow<'a, str>> {
    let value = row.get(col)?;
    if matches!(value, CellValue::Empty) {
        return None;
    }
    let mut key = String::new();
    append_key_part(&mut key, value, mode);
    Some(Cow::Owned(key))
}

/// 生成多列复合 key；空列跳过，整键为空返回 None。
///
/// 分隔符仍是历史约定的 U+0001，未做转义；因此包含该字符的复合键仍
/// 存在编码碰撞风险，不能在本次性能优化中隐式改变匹配契约。
fn make_composite_key<'a>(
    row: &'a [CellValue],
    cols: &[usize],
    mode: KeyMode,
) -> Option<Cow<'a, str>> {
    let mut key = String::new();
    let mut any_non_empty = false;
    for (i, &col) in cols.iter().enumerate() {
        if i != 0 {
            key.push('\u{1}');
        }
        let value = row.get(col)?;
        any_non_empty |= append_key_part(&mut key, value, mode);
    }
    any_non_empty.then_some(Cow::Owned(key))
}

/// 生成 key；单列连接走无中间容器的快速路径。
fn make_key<'a>(row: &'a [CellValue], cols: &[usize], mode: KeyMode) -> Option<Cow<'a, str>> {
    match cols {
        [] => None,
        [col] => make_single_key(row, *col, mode),
        _ => make_composite_key(row, cols, mode),
    }
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
    index.estimate(left, lk, join_type)
}

impl JoinIndex {
    fn estimate(&self, left: &Table, lk: &[usize], join_type: JoinType) -> JoinEstimate {
        let mut output_rows = 0usize;
        for row in &left.rows {
            let key = make_key(row, lk, self.mode);
            let matches = key.as_deref().and_then(|key| self.lookup(key));
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

fn output_row(
    left_row: &[CellValue],
    left_width: usize,
    right_row: Option<&[CellValue]>,
    right_pick: &[usize],
    output_width: usize,
) -> Vec<CellValue> {
    let mut output = Vec::with_capacity(output_width);
    output.extend(left_row.iter().take(left_width).cloned());
    output.resize(left_width, CellValue::Empty);
    match right_row {
        Some(right_row) => {
            for &col in right_pick {
                output.push(right_row.get(col).cloned().unwrap_or(CellValue::Empty));
            }
        }
        None => output.resize(output_width, CellValue::Empty),
    }
    output
}

fn run_join(
    left: &Table,
    right: &Table,
    spec: &JoinSpec,
    max_output_rows: Option<usize>,
    collect_timings: bool,
) -> Result<(JoinResult, JoinTimings), JoinLimitExceeded> {
    let started = Instant::now();
    let index_started = Instant::now();
    let index = JoinIndex::build(right, &spec.right_keys, spec.key_mode, spec.expand_dup);
    let mut timings = JoinTimings {
        index: index_started.elapsed(),
        ..JoinTimings::default()
    };

    // 有上限时在同一 JoinIndex 上预估，超限直接返回，避免第二次重建 B 索引。
    let estimated_output = if let Some(limit) = max_output_rows {
        let preflight_started = Instant::now();
        let estimate = index.estimate(left, &spec.left_keys, spec.join_type);
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
    let mut headers = left.headers.clone();
    for &col in &rp_valid {
        headers.push(right.headers[col].clone());
    }
    let left_width = left.col_count();
    let output_width = headers.len();
    let mut out = Table::new(headers);
    let reserve_rows = estimated_output.or_else(|| {
        (spec.join_type == JoinType::Left && !spec.expand_dup).then_some(left.rows.len())
    });
    if let Some(rows) = reserve_rows.filter(|&rows| rows < usize::MAX) {
        out.rows.reserve(rows);
    }
    let mut row_hit = Vec::new();
    if let Some(rows) = reserve_rows.filter(|&rows| rows < usize::MAX) {
        row_hit.reserve(rows);
    }

    let mut left_matched = 0usize;
    let mut right_used: Vec<bool> = vec![false; right.rows.len()];
    let measure = collect_timings;
    let mut probe_time = Duration::ZERO;
    let mut materialize_time = Duration::ZERO;

    // ── 左表驱动(left / inner) ──
    for row in &left.rows {
        let probe_started = measure.then(Instant::now);
        let key = make_key(row, &spec.left_keys, spec.key_mode);
        let hit = key.as_deref().and_then(|key| index.lookup(key));
        if let Some(probe_started) = probe_started {
            probe_time += probe_started.elapsed();
        }

        match hit {
            Some(matches) => {
                left_matched += 1;
                matches.for_each_selected(spec.expand_dup, |right_index| {
                    right_used[right_index] = true;
                    let materialize_started = measure.then(Instant::now);
                    let row = output_row(
                        row,
                        left_width,
                        right.rows.get(right_index).map(Vec::as_slice),
                        &rp_valid,
                        output_width,
                    );
                    out.push_row(row);
                    row_hit.push(true);
                    if let Some(materialize_started) = materialize_started {
                        materialize_time += materialize_started.elapsed();
                    }
                });
            }
            None if spec.join_type == JoinType::Left => {
                let materialize_started = measure.then(Instant::now);
                let row = output_row(row, left_width, None, &rp_valid, output_width);
                out.push_row(row);
                row_hit.push(false);
                if let Some(materialize_started) = materialize_started {
                    materialize_time += materialize_started.elapsed();
                }
            }
            None => {}
        }
    }

    timings.probe = probe_time;
    timings.materialize = materialize_time;
    timings.total = started.elapsed();

    let right_matched_rows = right_used.iter().filter(|&&used| used).count();
    let out_rows = out.row_count();
    Ok((
        JoinResult {
            table: out,
            left_total: left.rows.len(),
            left_matched,
            right_total: right.rows.len(),
            right_matched_rows,
            out_rows,
            row_hit,
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
            right_pick: vec![pick],
            key_mode: mode,
            expand_dup: true,
        }
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
        assert_eq!(r.table.cell(0, 2), Some(&CellValue::Text("eng".into())));
        assert_eq!(r.table.cell(1, 2), Some(&CellValue::Empty)); // bob 未匹配
        assert_eq!(r.table.cell(2, 2), Some(&CellValue::Text("ops".into())));
        assert_eq!(r.left_matched, 2);
    }

    #[test]
    fn inner_join_drops_unmatched() {
        let a = tbl(&["id"], &[&["1"], &["2"], &["3"]]);
        let b = tbl(&["id", "v"], &[&["2", "x"], &["3", "y"]]);
        let r = join(&a, &b, &spec(JoinType::Inner, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(r.table.row_count(), 2);
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("x".into())));
        assert_eq!(r.table.cell(1, 1), Some(&CellValue::Text("y".into())));
        // inner:未命中行被丢弃,输出全为命中行
        assert_eq!(r.row_hit, vec![true, true]);
    }

    #[test]
    fn row_hit_marks_left_unmatched() {
        // 左连接:匹配上的行 hit=true;未匹配 A 行(补空)hit=false
        let a = tbl(&["id"], &[&["1"], &["2"], &["3"]]);
        let b = tbl(&["id", "v"], &[&["1", "x"], &["3", "y"]]);
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(r.table.row_count(), 3);
        assert_eq!(r.row_hit, vec![true, false, true]);
    }

    #[test]
    fn duplicate_right_key_fanout() {
        // 右表同 key 两行 → left 行复制成两行(vlookup 对重复键取第一条,这里取全部)
        let a = tbl(&["id"], &[&["1"]]);
        let b = tbl(&["id", "v"], &[&["1", "a"], &["1", "b"]]);
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(r.table.row_count(), 2);
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("a".into())));
        assert_eq!(r.table.cell(1, 1), Some(&CellValue::Text("b".into())));
        // 展开的两行都算命中
        assert_eq!(r.row_hit, vec![true, true]);
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
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("a".into())));
        assert_eq!(r.row_hit, vec![true]);
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
                right_pick: vec![2],
                key_mode: KeyMode::EXACT,
                expand_dup: true,
            },
        );
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(r.table.headers, vec!["k1", "k2", "v", "w"]);
        assert_eq!(r.table.cell(0, 3), Some(&CellValue::Text("r1".into())));
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
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("num".into())));

        // Exact 下数字 123 ≠ 文本 "123"
        let r2 = join(&a, &tb, &spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(r2.table.cell(0, 1), Some(&CellValue::Empty));
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
        assert_eq!(result.table.cell(0, 1), Some(&CellValue::Empty));
    }

    #[test]
    fn empty_key_not_matched() {
        // 左表空键行 → left 保留但右侧空
        let a = tbl(&["id"], &[&[""], &["1"]]);
        let b = tbl(&["id", "v"], &[&["1", "x"]]);
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(r.table.row_count(), 2);
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Empty));
        assert_eq!(r.table.cell(1, 1), Some(&CellValue::Text("x".into())));
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
                right_pick: vec![1, 2],
                key_mode: KeyMode::EXACT,
                expand_dup: true,
            },
        );
        assert_eq!(r.table.headers, vec!["id", "x", "y"]);
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(r.table.cell(0, 2), Some(&CellValue::Text("20".into())));
    }

    #[test]
    fn bracket_fold_matches_chinese_english() {
        // 开启括号归一化:中文（）与英文 () 互认
        let a = tbl(&["name"], &[&["苹果（红）"]]);
        let b = tbl(&["name", "v"], &[&["苹果(红)", "x"]]);
        let m = KeyMode {
            number_text: false,
            brackets: true,
        };
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m));
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("x".into())));

        // 关闭括号归一化则不匹配
        let m2 = KeyMode {
            number_text: false,
            brackets: false,
        };
        let r2 = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m2));
        assert_eq!(r2.table.row_count(), 1);
        assert_eq!(r2.table.cell(0, 1), Some(&CellValue::Empty));
    }

    #[test]
    fn bracket_fold_covers_all_pairs() {
        // 【】→[]、｛｝→{} 等成对折叠
        let a = tbl(&["k"], &[&["型号【A】｛B｝〔C〕"]]);
        let b = tbl(&["k", "v"], &[&["型号[A]{B}(C)", "hit"]]);
        let m = KeyMode {
            number_text: false,
            brackets: true,
        };
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m));
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("hit".into())));
    }

    #[test]
    fn bracket_fold_independent_of_number_text() {
        // 括号归一化与数字/文本互认是独立开关:开括号时数字仍精确
        let a = tbl(&["id"], &[&["1（a）"]]);
        let b = tbl(&["id", "v"], &[&["1(a)", "x"]]);
        let m = KeyMode {
            number_text: false,
            brackets: true,
        };
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m));
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("x".into())));
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
        assert_eq!(exact.row_hit, vec![false, true, false]);

        let normalized = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::NORMALIZE));
        // Text("") 与纯空白在 trim 后相同，但仍不与 Empty 相同。
        assert_eq!(normalized.row_hit, vec![false, true, true]);
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
                right_pick: vec![2],
                key_mode: KeyMode::EXACT,
                expand_dup: true,
            },
        );
        assert_eq!(
            result.table.cell(0, 2),
            Some(&CellValue::Text("hit".into()))
        );
    }
}
