//! Join 引擎:实现 VLOOKUP(left join)与 inner/right/full join
//!
//! 左表按「原列」输出;右表只输出「取值列」,避免键列重复。
//! 键支持多列复合;归一化(trim、数字/文本互认)可配置。

use std::collections::HashMap;

use crate::model::{CellValue, Table};

/// join 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    /// VLOOKUP 语义:保留左表所有行,匹配不到则右侧补空
    Left,
    /// 只保留两表都能匹配的行
    Inner,
    /// 保留右表所有行(右表行序),匹配不到左表补空
    Right,
    /// 两表所有行,未匹配侧补空(left 行先,right-only 行在后)
    Full,
}

impl JoinType {
    pub fn label(&self) -> &'static str {
        match self {
            JoinType::Left => "左连接(VLOOKUP)",
            JoinType::Inner => "内连接(交集)",
            JoinType::Right => "右连接",
            JoinType::Full => "全连接(并集)",
        }
    }
    pub fn all() -> [JoinType; 4] {
        [JoinType::Left, JoinType::Inner, JoinType::Right, JoinType::Full]
    }
}

/// 键归一化策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMode {
    /// 精确:数字 1 与文本 "1" 视为不同
    Exact,
    /// 宽松:数字与同文本视为相同,并 trim 首尾空白
    Normalize,
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
            match mode {
                // 精确:数字加前缀,与文本区分
                KeyMode::Exact => format!("N:{d}"),
                // 宽松:数字与同文本互认 → 与文本走同一格式
                KeyMode::Normalize => format!("V:{d}"),
            }
        }
        CellValue::Text(s) => match mode {
            KeyMode::Exact => format!("S:{s}"),
            // 宽松:trim 后与数字同格式
            KeyMode::Normalize => format!("V:{}", s.trim()),
        },
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

    // ── 左表驱动(left / inner;full 也过左表;right 特殊:以右表为主) ──
    if spec.join_type != JoinType::Right {
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
                }
            }
        }
    } else {
        // ── Right join:以右表为主序 ──
        let lcols = left.col_count();
        // 左表 key → 行(取第一个命中即可,与 left 相反方向)
        let mut left_index: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, row) in left.rows.iter().enumerate() {
            if let Some(k) = make_key(row, lk, spec.key_mode) {
                left_index.entry(k).or_default().push(i);
            }
        }
        for (i, row) in right.rows.iter().enumerate() {
            let key = make_key(row, rk, spec.key_mode);
            let hit = key.as_ref().and_then(|k| left_index.get(k));
            match hit {
                Some(idxs) if !idxs.is_empty() => {
                    right_used[i] = true;
                    left_matched += idxs.len();
                    for &li in idxs {
                        // 左表行 + 右表取值
                        let mut o = left.rows[li].clone();
                        if has_pick {
                            o.extend(pick_extra(i));
                        }
                        out.push_row(o);
                    }
                }
                _ => {
                    if make_key(row, rk, spec.key_mode).is_none() {
                        continue; // 无键行跳过
                    }
                    let mut o: Vec<CellValue> = vec![CellValue::Empty; lcols];
                    if has_pick {
                        o.extend(pick_extra(i));
                    }
                    out.push_row(o);
                }
            }
        }
    }

    // ── 右表独有行(full 才补;right 已在主循环处理) ──
    if spec.join_type == JoinType::Full {
        let lcols = left.col_count();
        for (i, row) in right.rows.iter().enumerate() {
            if right_used[i] {
                continue;
            }
            // 无键行不参与 right-only(它从未可能匹配)
            if make_key(row, rk, spec.key_mode).is_none() {
                continue;
            }
            let mut o: Vec<CellValue> = vec![CellValue::Empty; lcols];
            if has_pick {
                o.extend(pick_extra(i));
            }
            out.push_row(o);
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
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::Exact));
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
        let r = join(&a, &b, &spec(JoinType::Inner, 0, 0, 1, KeyMode::Exact));
        assert_eq!(r.table.row_count(), 2);
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("x".into())));
        assert_eq!(r.table.cell(1, 1), Some(&CellValue::Text("y".into())));
    }

    #[test]
    fn right_join_keeps_right_order() {
        let a = tbl(&["id"], &[&["1"], &["2"]]);
        let b = tbl(&["id", "v"], &[&["9", "z"], &["2", "x"]]);
        let r = join(&a, &b, &spec(JoinType::Right, 0, 0, 1, KeyMode::Exact));
        // 右表顺序:9(z, 左空) → 2(x)
        assert_eq!(r.table.row_count(), 2);
        assert_eq!(r.table.cell(0, 0), Some(&CellValue::Empty));
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("z".into())));
        assert_eq!(r.table.cell(1, 0), Some(&CellValue::Text("2".into())));
    }

    #[test]
    fn full_join_keeps_right_only() {
        let a = tbl(&["id"], &[&["1"], &["2"]]);
        let b = tbl(&["id", "v"], &[&["2", "x"], &["9", "z"]]);
        let r = join(&a, &b, &spec(JoinType::Full, 0, 0, 1, KeyMode::Exact));
        assert_eq!(r.table.row_count(), 3);
        assert_eq!(r.table.cell(2, 0), Some(&CellValue::Empty));
        assert_eq!(r.table.cell(2, 1), Some(&CellValue::Text("z".into())));
    }

    #[test]
    fn duplicate_right_key_fanout() {
        // 右表同 key 两行 → left 行复制成两行(vlookup 对重复键取第一条,这里取全部)
        let a = tbl(&["id"], &[&["1"]]);
        let b = tbl(&["id", "v"], &[&["1", "a"], &["1", "b"]]);
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::Exact));
        assert_eq!(r.table.row_count(), 2);
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("a".into())));
        assert_eq!(r.table.cell(1, 1), Some(&CellValue::Text("b".into())));
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
                key_mode: KeyMode::Exact,
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
        let r = join(&a, &tb, &spec(JoinType::Left, 0, 0, 1, KeyMode::Normalize));
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(r.table.cell(0, 1), Some(&CellValue::Text("num".into())));

        // Exact 下数字 123 ≠ 文本 "123"
        let r2 = join(&a, &tb, &spec(JoinType::Left, 0, 0, 1, KeyMode::Exact));
        assert_eq!(r2.table.cell(0, 1), Some(&CellValue::Empty));
    }

    #[test]
    fn empty_key_not_matched() {
        // 左表空键行 → left 保留但右侧空
        let a = tbl(&["id"], &[&[""], &["1"]]);
        let b = tbl(&["id", "v"], &[&["1", "x"]]);
        let r = join(&a, &b, &spec(JoinType::Left, 0, 0, 1, KeyMode::Exact));
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
                key_mode: KeyMode::Exact,
            },
        );
        assert_eq!(r.table.headers, vec!["id", "x", "y"]);
        assert_eq!(r.table.row_count(), 1);
        assert_eq!(r.table.cell(0, 2), Some(&CellValue::Text("20".into())));
    }
}
