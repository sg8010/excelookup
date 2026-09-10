//! Join release benchmark。
//!
//! 由 scripts/bench-join.sh 编译，同一驱动适配原始基线和当前核心库。
//! 不传 case 时依次运行全部场景；建议用 `/usr/bin/time -v` 包住每个场景
//! 单独进程，以得到该场景的峰值 RSS。

use std::alloc::{GlobalAlloc, Layout, System};
use std::env;
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use excelookup_lib::join::{JoinSpec, JoinType, KeyMode, join_with_limit};
use excelookup_lib::model::{CellValue, Table};

struct CountingAllocator;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[cfg(count_allocations)]
#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

const MAX_EXPAND_ROWS: usize = 5_000_000;

#[derive(Clone, Copy, Debug)]
enum DuplicateShape {
    Unique,
    TenPercent,
    High,
}

#[derive(Clone, Copy, Debug)]
enum KeyData {
    Text,
    NumberText,
    Brackets,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    name: &'static str,
    left_rows: usize,
    right_rows: usize,
    duplicate_shape: DuplicateShape,
    match_percent: usize,
    text_len: usize,
    width: usize,
    key_data: KeyData,
    join_type: JoinType,
    expand_dup: bool,
}

fn cases() -> &'static [Case] {
    &[
        Case {
            name: "u100k_exact_narrow_100",
            left_rows: 100_000,
            right_rows: 100_000,
            duplicate_shape: DuplicateShape::Unique,
            match_percent: 100,
            text_len: 8,
            width: 2,
            key_data: KeyData::Text,
            join_type: JoinType::Left,
            expand_dup: true,
        },
        Case {
            name: "u500k_exact_narrow_90",
            left_rows: 500_000,
            right_rows: 500_000,
            duplicate_shape: DuplicateShape::Unique,
            match_percent: 90,
            text_len: 8,
            width: 2,
            key_data: KeyData::Text,
            join_type: JoinType::Left,
            expand_dup: true,
        },
        Case {
            name: "u1m_normalize_wide_80_noexpand",
            left_rows: 1_000_000,
            right_rows: 1_000_000,
            duplicate_shape: DuplicateShape::Unique,
            match_percent: 80,
            text_len: 8,
            width: 8,
            key_data: KeyData::NumberText,
            join_type: JoinType::Left,
            expand_dup: false,
        },
        Case {
            name: "d10_500k_normalize_90_expand",
            left_rows: 500_000,
            right_rows: 500_000,
            duplicate_shape: DuplicateShape::TenPercent,
            match_percent: 90,
            text_len: 8,
            width: 4,
            key_data: KeyData::NumberText,
            join_type: JoinType::Left,
            expand_dup: true,
        },
        Case {
            name: "d10_100k_exact_50_noexpand",
            left_rows: 100_000,
            right_rows: 100_000,
            duplicate_shape: DuplicateShape::TenPercent,
            match_percent: 50,
            text_len: 16,
            width: 4,
            key_data: KeyData::Text,
            join_type: JoinType::Left,
            expand_dup: false,
        },
        Case {
            name: "high_500k_bracket_100_expand",
            left_rows: 5_000,
            right_rows: 500_000,
            duplicate_shape: DuplicateShape::High,
            match_percent: 100,
            text_len: 64,
            width: 8,
            key_data: KeyData::Brackets,
            join_type: JoinType::Left,
            expand_dup: true,
        },
        Case {
            name: "high_500k_bracket_50_noexpand",
            left_rows: 500_000,
            right_rows: 500_000,
            duplicate_shape: DuplicateShape::High,
            match_percent: 50,
            text_len: 64,
            width: 8,
            key_data: KeyData::Brackets,
            join_type: JoinType::Left,
            expand_dup: false,
        },
        Case {
            name: "bracket_100k_exact_75",
            left_rows: 100_000,
            right_rows: 100_000,
            duplicate_shape: DuplicateShape::Unique,
            match_percent: 75,
            text_len: 64,
            width: 4,
            key_data: KeyData::Brackets,
            join_type: JoinType::Left,
            expand_dup: true,
        },
        Case {
            name: "d10_100k_normalize_inner_70",
            left_rows: 100_000,
            right_rows: 100_000,
            duplicate_shape: DuplicateShape::TenPercent,
            match_percent: 70,
            text_len: 16,
            width: 4,
            key_data: KeyData::NumberText,
            join_type: JoinType::Inner,
            expand_dup: false,
        },
        Case {
            name: "high_500k_expand_reject",
            left_rows: 100_000,
            right_rows: 500_000,
            duplicate_shape: DuplicateShape::High,
            match_percent: 100,
            text_len: 16,
            width: 2,
            key_data: KeyData::Text,
            join_type: JoinType::Left,
            expand_dup: true,
        },
    ]
}

fn distinct_count(case: Case) -> usize {
    match case.duplicate_shape {
        DuplicateShape::Unique => case.right_rows,
        DuplicateShape::TenPercent => case.right_rows * 9 / 10,
        DuplicateShape::High => (case.right_rows / 100).max(1),
    }
}

/// 生成可区分的短/长文本键。括号场景让 B 使用英文、A 使用中文，
/// 从而实际走括号归一化路径。
fn text_key(id: usize, text_len: usize, bracketed: bool, chinese_brackets: bool) -> String {
    let mut id_width = if bracketed {
        text_len.saturating_sub(4).max(6).min(18)
    } else {
        text_len.saturating_sub(1).max(6)
    };
    // 这些 benchmark 的最大 id 不超过 1e6；保证短键也不会因截断丢失唯一性。
    id_width = id_width.max(7);
    let mut key = format!("K{id:0id_width$}");
    if bracketed {
        let target = text_len.saturating_sub(3).max(key.len());
        while key.len() < target {
            key.push('x');
        }
        key.push(if chinese_brackets { '（' } else { '(' });
        key.push('x');
        key.push(if chinese_brackets { '）' } else { ')' });
    }
    key
}

fn key_cell(case: Case, id: usize, left: bool, matched: bool) -> CellValue {
    let id = if matched {
        id
    } else {
        distinct_count(case) + id
    };
    match case.key_data {
        KeyData::NumberText if left && matched => CellValue::Text(id.to_string()),
        KeyData::NumberText if !left => CellValue::Number(id as f64),
        KeyData::NumberText => CellValue::Text(format!("X{id}")),
        KeyData::Brackets => CellValue::Text(text_key(id, case.text_len, true, left)),
        KeyData::Text => CellValue::Text(text_key(id, case.text_len, false, false)),
    }
}

fn build_tables(case: Case) -> (Table, Table) {
    let distinct = distinct_count(case);
    let mut right = Table::new((0..case.width).map(|i| format!("b{i}")).collect());
    right.rows.reserve(case.right_rows);
    for row_index in 0..case.right_rows {
        let id = row_index % distinct;
        let mut row = Vec::with_capacity(case.width);
        row.push(key_cell(case, id, false, true));
        for col in 1..case.width {
            row.push(CellValue::Text(format!("r{col}_{row_index}")));
        }
        right.push_row(row);
    }

    let mut left = Table::new((0..case.width).map(|i| format!("a{i}")).collect());
    left.rows.reserve(case.left_rows);
    let matched_rows = case.left_rows * case.match_percent / 100;
    for row_index in 0..case.left_rows {
        let id = row_index % distinct;
        let matched = row_index < matched_rows;
        let mut row = Vec::with_capacity(case.width);
        row.push(key_cell(case, id, true, matched));
        for col in 1..case.width {
            row.push(CellValue::Text(format!("a{col}_{row_index}")));
        }
        left.push_row(row);
    }
    (left, right)
}

fn reset_allocations() {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn run_case(case: Case) {
    eprintln!(
        "正在生成 {}（A={}，B={}）",
        case.name, case.left_rows, case.right_rows
    );
    let (left, right) = build_tables(case);
    let spec = JoinSpec {
        join_type: case.join_type,
        left_keys: vec![0],
        right_keys: vec![0],
        right_pick: (1..case.width).collect(),
        key_mode: match case.key_data {
            KeyData::Text => KeyMode::EXACT,
            KeyData::NumberText => KeyMode::NORMALIZE,
            KeyData::Brackets => KeyMode {
                number_text: false,
                brackets: true,
            },
        },
        expand_dup: case.expand_dup,
    };
    let max_rows = case.expand_dup.then_some(MAX_EXPAND_ROWS);

    // 两版使用同一驱动；纯计时构建使用系统分配器，计数构建独立进程运行。
    reset_allocations();
    let started = Instant::now();
    let (actual, estimated) = match join_with_limit(&left, &right, &spec, max_rows) {
        Ok(result) => (Some(result), None),
        Err(error) => (None, Some(error.estimate.output_rows)),
    };
    let elapsed = started.elapsed();
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);

    // 独立地从生成规则计算行数、命中标志、统计及每条输出内容，不只比较两次实现。
    let distinct = distinct_count(case);
    let matched = case.left_rows * case.match_percent / 100;
    let mut expected_rows = 0usize;
    for i in 0..case.left_rows {
        expected_rows += if i < matched {
            if case.expand_dup {
                (case.right_rows - 1 - i % distinct) / distinct + 1
            } else {
                1
            }
        } else if case.join_type == JoinType::Left {
            1
        } else {
            0
        };
    }
    let rejected = expected_rows > max_rows.unwrap_or(usize::MAX);
    assert_eq!(actual.is_none(), rejected, "超限拒绝不符合生成规则");
    if let Some(result) = &actual {
        assert_eq!(result.out_rows, expected_rows, "实际输出行数错误");
        assert_eq!(result.left_matched, matched);
        let mut output = 0;
        let mut used = vec![false; right.rows.len()];
        for (i, row) in left.rows.iter().enumerate() {
            if i < matched {
                let indices = (i % distinct..right.rows.len()).step_by(distinct);
                for ri in indices.take(if case.expand_dup { usize::MAX } else { 1 }) {
                    assert!(result.row_hit[output]);
                    assert_eq!(&result.table.rows[output][..case.width], row);
                    assert_eq!(
                        &result.table.rows[output][case.width..],
                        &right.rows[ri][1..]
                    );
                    used[ri] = true;
                    output += 1;
                }
            } else if case.join_type == JoinType::Left {
                assert!(!result.row_hit[output]);
                assert_eq!(&result.table.rows[output][..case.width], row);
                assert!(
                    result.table.rows[output][case.width..]
                        .iter()
                        .all(|v| *v == CellValue::Empty)
                );
                output += 1;
            }
        }
        assert_eq!(
            result.right_matched_rows,
            used.iter().filter(|&&v| v).count()
        );
    }
    black_box(&actual);
    println!(
        "场景={} 版本={} 计数={} A行={} B行={} 预估行={:?} 实际行={} 拒绝={} 耗时ms={:.3} 分配次数={} 分配请求字节={}",
        case.name,
        if cfg!(benchmark_baseline) {
            "基线"
        } else {
            "优化"
        },
        cfg!(count_allocations),
        case.left_rows,
        case.right_rows,
        estimated,
        actual.as_ref().map_or(0, |r| r.out_rows),
        rejected,
        milliseconds(elapsed),
        allocations,
        bytes
    );
}

fn main() {
    let wanted = env::args().nth(1);
    let selected = cases()
        .iter()
        .copied()
        .filter(|case| wanted.as_deref().is_none_or(|name| name == case.name));
    let mut found = false;
    for case in selected {
        found = true;
        run_case(case);
    }
    if !found {
        eprintln!("未找到场景；可用场景：");
        for case in cases() {
            eprintln!("  {}", case.name);
        }
        std::process::exit(2);
    }
}
