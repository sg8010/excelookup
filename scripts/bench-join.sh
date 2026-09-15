#!/usr/bin/env bash
# 同一驱动、同一编译参数；每次采样独立进程，计时与分配统计分离。
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="${HOME}/.cargo/bin:${PATH}"
baseline_source="scripts/bench-baseline"
[[ -f "$baseline_source/join.rs" && -f "$baseline_source/model.rs" ]] || {
  echo "缺少本轮基线源码快照: $baseline_source" >&2
  exit 1
}
bench_dir=$(mktemp -d "${TMPDIR:-/tmp}/excelookup-bench.XXXXXX")
echo "基准产物与原始日志：$bench_dir"
cp scripts/join_bench.rs "$bench_dir/driver.rs"
rustc --version > "$bench_dir/environment.txt"
uname -a >> "$bench_dir/environment.txt"
lscpu >> "$bench_dir/environment.txt"
git rev-parse HEAD >> "$bench_dir/environment.txt"
printf 'baseline_source=%s\n' "$baseline_source" >> "$bench_dir/environment.txt"
# src/join.rs 依赖 foldhash crate；本脚本绕开 cargo 用 rustc 直编，需先让
# cargo 产出 foldhash rlib，编译 current 一侧时显式 --extern。baseline 快照
# 是 foldhash 之前的旧实现，无此依赖。
cargo build --release --package foldhash >/dev/null
foldhash_dir="${CARGO_TARGET_DIR:-$PWD/target}/release/deps"
foldhash_rlib=$(ls -t "$foldhash_dir"/libfoldhash-*.rlib 2>/dev/null | head -1)
[[ -n "$foldhash_rlib" && -f "$foldhash_rlib" ]] || {
  echo "未找到 foldhash rlib：cargo build --release --package foldhash 未产出" >&2
  exit 1
}
printf 'foldhash_rlib=%s\n' "$foldhash_rlib" >> "$bench_dir/environment.txt"
for version in baseline current; do
  mkdir -p "$bench_dir/$version"
  for source in join model; do
    if [[ $version == baseline ]]; then
      cp "$baseline_source/$source.rs" "$bench_dir/$version/$source.rs"
    else
      cp "src/$source.rs" "$bench_dir/$version/$source.rs"
    fi
  done
  cat > "$bench_dir/$version/lib.rs" <<'RS'
pub mod join;
pub mod model;
RS
  # current 的 join.rs 依赖 foldhash：编译 rlib 时显式 --extern；编译 driver 时
  # rustc 读 rlib 元数据还要能定位 foldhash，经 -L dependency 搜索路径提供。
  lib_externs=()
  dep_flags=()
  if [[ $version == current ]]; then
    lib_externs+=(--extern "foldhash=$foldhash_rlib")
    dep_flags+=(-L "dependency=$foldhash_dir")
  fi
  rustc --edition=2024 -O --crate-name excelookup_lib --crate-type rlib \
    "${lib_externs[@]}" "$bench_dir/$version/lib.rs" -o "$bench_dir/$version/libexcelookup_lib.rlib"
  config=()
  if [[ $version == baseline ]]; then config+=(--cfg benchmark_baseline); fi
  for mode in timing allocations; do
    counts=()
    if [[ $mode == allocations ]]; then counts+=(--cfg count_allocations); fi
    rustc --edition=2024 -O -A dead_code "${config[@]}" "${counts[@]}" "${dep_flags[@]}" \
      "$bench_dir/driver.rs" --extern "excelookup_lib=$bench_dir/$version/libexcelookup_lib.rlib" \
      -o "$bench_dir/$version/$mode"
  done
done
diff -u "$bench_dir/baseline/join.rs" "$bench_dir/current/join.rs" > "$bench_dir/round.diff" || true
cases=(
  u100k_exact_narrow_100 u500k_exact_narrow_90 u1m_normalize_wide_80_noexpand
  d10_500k_normalize_90_expand d10_100k_exact_50_noexpand
  high_500k_bracket_100_expand high_500k_bracket_50_noexpand
  bracket_100k_exact_75 d10_100k_normalize_inner_70 high_500k_expand_reject
)
if (( $# > 0 )); then cases=("$@"); fi
for case_name in "${cases[@]}"; do
  # 交替运行两版，降低运行顺序对结果的影响；报告三次纯计时的中位数。
  for repeat in 1 2 3; do
    versions=(baseline current)
    if (( repeat % 2 == 0 )); then versions=(current baseline); fi
    for version in "${versions[@]}"; do
      /usr/bin/time -f '峰值RSS_KB=%M' -o "$bench_dir/$case_name.$version.$repeat.rss" \
        "$bench_dir/$version/timing" "$case_name" \
        | tee "$bench_dir/$case_name.$version.$repeat.txt"
    done
  done
  for version in baseline current; do
    "$bench_dir/$version/allocations" "$case_name" \
      | tee "$bench_dir/$case_name.$version.allocations.txt"
  done
done
