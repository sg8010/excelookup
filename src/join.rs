//! Join 引擎:实现 VLOOKUP(left join)与 inner join
//!
//! 左表按「原列」输出;右表只输出「取值列」,避免键列重复。
//! 键支持多列复合;归一化可配置:数字/文本互认+trim、中文/英文括号互认。

use std::collections::HashMap;

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
    pub const EXACT: Self = Self { number_text: false, brackets: false };
    /// 仅数字/文本互认 + trim(不带括号归一化)
    pub const NORMALIZE: Self = Self { number_text: true, brackets: false };
}

impl Default for KeyMode {
    fn default() -> Self {
        // 默认:两开关全开(数字/文本互认 + 括号归一化)
        Self { number_text: true, brackets: true }
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

/// 中文括号 → 对应英文括号(括号归一化)
fn fold_brackets(s: &str) -> String {
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
        .collect()
}

/// 生成单列 key 片段
fn key_part(v: &CellValue, mode: KeyMode) -> String {
    match v {
        CellValue::Empty => String::new(),
        CellValue::Number(n) => {
            let d = if n.fract() == 0.0 {
                format!("{:.0}", n)
            } else {
                n.to_string()
            };
            if mode.number_text {
                // 宽松:数字与同文本互认 → 与文本走同一格式
                format!("V:{d}")
            } else {
                // 精确:数字加前缀,与文本区分
                format!("N:{d}")
            }
        }
        CellValue::Text(s) => {
            let mut body: String = if mode.brackets {
                fold_brackets(s)
            } else {
                s.clone()
            };
            if mode.number_text {
                body = body.trim().to_string();
            }
            if mode.number_text {
                // 宽松:trim 后与数字同格式
                format!("V:{body}")
            } else {
                format!("S:{body}")
            }
        }
    }
}

/// 生成多列复合 key;空列跳过;全空返回 None
fn make_key(row: &[CellValue], cols: &[usize], mode: KeyMode) -> Option<String> {
    if cols.is_empty() {
        return None;
    }
    let mut parts = Vec::with_capacity(cols.len());
    for &c in cols {
        let v = row.get(c)?;
        parts.push(key_part(v, mode));
    }
    if parts.iter().all(|p| p.is_empty()) {
        return None; // 整键为空 → 不参与匹配
    }
    Some(parts.join("\u{1}"))
}

pub fn join(left: &Table, right: &Table, spec: &JoinSpec) -> JoinResult {
    let lk = &spec.left_keys;
    let rk = &spec.right_keys;

    // 右表取值列(过滤越界)
    let rp_valid: Vec<usize> = spec
        .right_pick
        .iter()
        .copied()
        .filter(|&c| c < right.col_count())
        .collect();
    // 右表取值列在输出中是否还有意义(非空说明要拼)
    let has_pick = !rp_valid.is_empty();

    // 索引:右表 key → 行下标(允许一右对多左)
    let mut index: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, row) in right.rows.iter().enumerate() {
        if let Some(k) = make_key(row, rk, spec.key_mode) {
            index.entry(k).or_default().push(i);
        }
    }

    // 输出表 = 左表头 + 右取值列表头
    let mut headers = left.headers.clone();
    if has_pick {
        for &c in &rp_valid {
            headers.push(right.headers[c].clone());
        }
    }
    let mut out = Table::new(headers);
    let mut row_hit: Vec<bool> = Vec::new();

    let mut left_matched = 0usize;
    let mut right_used: Vec<bool> = vec![false; right.rows.len()];

    let pick_extra = |ri: usize| -> Vec<CellValue> {
        let mut v = Vec::with_capacity(rp_valid.len());
        for &c in &rp_valid {
            v.push(
                right
                    .rows
                    .get(ri)
                    .and_then(|r| r.get(c))
                    .cloned()
                    .unwrap_or(CellValue::Empty),
            );
        }
        v
    };

    // ── 左表驱动(left / inner) ──
    for row in &left.rows {
        let key = make_key(row, lk, spec.key_mode);
        let hit_idxs = key.as_ref().and_then(|k| index.get(k));

        match hit_idxs {
            Some(idxs) if !idxs.is_empty() => {
                left_matched += 1;
                for &ri in idxs {
                    right_used[ri] = true;
                    let mut o = row.clone();
                    if has_pick {
                        o.extend(pick_extra(ri));
                    }
                    out.push_row(o);
                    row_hit.push(true);
                }
            }
            // 无匹配
            _ => {
                if spec.join_type == JoinType::Inner {
                    continue; // inner:丢弃
                }
                let mut o = row.clone();
                if has_pick {
                    o.resize(o.len() + rp_valid.len(), CellValue::Empty);
                }
                out.push_row(o);
                row_hit.push(false);
            }
        }
    }

    let right_matched_rows = right_used.iter().filter(|&&b| b).count();
    let out_rows = out.row_count();
    JoinResult {
        table: out,
        left_total: left.rows.len(),
        left_matched,
        right_total: right.rows.len(),
        right_matched_rows,
        out_rows,
        row_hit,
    }
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
        }
    }

    #[test]
    fn left_join_vlookup() {
        let a = tbl(&["id", "name"], &[&["1", "alice"], &["2", "bob"], &["3", "carol"]]);
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
        tb.push_row(vec![CellValue::Number(123.0), CellValue::Text("num".into())]);
        let r = join(&a, &tb, &spec(JoinType::Left, 0, 0, 1, KeyMode::NORMALIZE));
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("num".into())));

        // Exact 下数字 123 ≠ 文本 "123"
        let r2 = join(&a, &tb, &spec(JoinType::Left, 0, 0, 1, KeyMode::EXACT));
        assert_eq!(r2.table.cell(0, 1), Some(&CellValue::Empty));
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
        let m = KeyMode { number_text: false, brackets: true };
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m));
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("x".into())));

        // 关闭括号归一化则不匹配
        let m2 = KeyMode { number_text: false, brackets: false };
        let r2 = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m2));
        assert_eq!(r2.table.row_count(), 1);
        assert_eq!(r2.table.cell(0, 1), Some(&CellValue::Empty));
    }

    #[test]
    fn bracket_fold_covers_all_pairs() {
        // 【】→[]、｛｝→{} 等成对折叠
        let a = tbl(&["k"], &[&["型号【A】｛B｝〔C〕"]]);
        let b = tbl(&["k", "v"], &[&["型号[A]{B}(C)", "hit"]]);
        let m = KeyMode { number_text: false, brackets: true };
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m));
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("hit".into())));
    }

    #[test]
    fn bracket_fold_independent_of_number_text() {
        // 括号归一化与数字/文本互认是独立开关:开括号时数字仍精确
        let a = tbl(&["id"], &[&["1（a）"]]);
        let b = tbl(&["id", "v"], &[&["1(a)", "x"]]);
        let m = KeyMode { number_text: false, brackets: true };
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, m));
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("x".into())));
    }
}
