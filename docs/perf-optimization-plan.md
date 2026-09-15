# 性能与内存优化方案（评审后）

本文档记录本轮代码评审确认的 6 项优化，每项按「问题现状 / 原因分析 / 优化措施 /
影响范围 / 预期收益 / 验证方法 / 风险点」展开。评审结论：先做 1、3、4、11，再
按实测落地 9、2；已淘汰的项（可见行数预估的纯融合版、Box<str> 索引键、Table
扁平化、vendor patch rust_xlsxwriter）记录在本节末尾，避免重复评估。

性能数字来自两套测量：

- join 微基准：`/tmp/exl-bench` 下的多变体基准（同一 `scripts/join_bench.rs`
  驱动、同一 rustc `-O` 编译，每场景每变体 3 次独立进程取中位数，交替运行
  顺序）。变体：base（当前代码）、fuse（纯单遍融合）、fuse2（上界跳过预估，
  即本文 #2 修改后方案）、fh（foldhash）、f2fh（fuse2+foldhash）。
- 结构尺寸实测：`CellValue` = 24 B，`JoinedRow{usize, Option<usize>}` = 24 B，
  `Option<usize>` = 16 B，`RowMatches` = 24 B，`JoinedRow{u32,u32}` = 8 B。

注意：`docs/join-benchmark.md` 中的耗时是 `JoinedTable` 行引用改造**之前**的
数据，不代表当前版本，不能用于推算本轮收益。

## 1. Join 后台化

**问题现状**
`run_join`（src/app.rs:904）在按钮点击处同步调用 `join_with_limit`
（src/app.rs:2360）。索引构建、预估、连接、命中统计全部跑在 egui UI 线程上；
500k 行 join 当前实测 ~200ms，1M 行 ~470ms，期间整个界面无响应。

**原因分析**
egui 每帧绘制都经过 `App::ui()`；任何同步的重计算都会阻塞帧循环。加载和导出
已经用「后台线程 + mpsc channel + 世代号」模式解决了同类问题，join 没有。

**优化措施**
仿照 `start_load`/`start_export` 的模式：新增 `JoinMsg{generation, result}`、
`join_rx/join_tx` channel、`join_gen` 世代号、`join_active` 标志。`run_join`
只 Arc 克隆 A/B 当前表和参数后 spawn 线程（`catch_unwind` 兜底），线程内执行
`join_with_limit` 并算好 `matched_rows`/`unmatched_rows`，结果经 channel 回传；
帧末 `poll_join` 校验世代号后装配 `JoinOutcome`。`join_active` 期间按钮置灰
防重入——世代号只保证旧结果不落回，并不会停止旧线程的计算。

**影响范围**
仅 `src/app.rs`：`run_join`、结果装配、`ui_config_body` 的「执行连接」按钮、
帧末 poll 段。lib 层接口不变。

**预期收益**
join 本身耗时不变；消除 UI 冻结，大表 join 期间界面保持可交互。

**验证方法**
`cargo test`（lib 无改动，全量回归）；手动验证：加载大文件执行连接，期间滚动/
切换步骤应无卡顿，连接完成后结果正常展示。按 AGENTS.md 约定，GUI 端到端由用
户手测。

**风险点**
- 结果视图持有 `Arc<Table>` 快照，join 期间换源/切表使旧结果作废即可，快照保
  证计算中数据不被释放。
- `JoinResult` 内部均为普通数据（`Vec`/`String`/`usize`），满足 `Send`。
- `join_active` 同时只能跑一个 join，避免多个大任务叠加占内存。

## 3. 结果筛选行号缓存

**问题现状**
`ui_result_table`（src/app.rs:2821-2835）在行筛选（已匹配/未命中）激活时，每
帧都对 `table.rows` 做一次 O(N) 扫描并 collect 出 `Vec<usize>`。500 万行结果
每帧分配 ~40MB 并全扫一遍，虚拟滚动只能省渲染、省不掉这份重建。

**原因分析**
筛选结果是绘制期的纯函数输出，没有缓存；帧与帧之间数据未变也要重算。

**优化措施**
把缓存挂在结果版本上：给 `JoinOutcome` 增加递增的 `result_id`（或直接比较
`Arc::ptr_eq(&result.table)`），另存 `filter_cache: Option<(usize, RowFilter,
Vec<usize>)>`。`ui_result_table` 命中缓存直接用，未命中才重建一次。结果替换
（`result = None`/新 run_join）时缓存随之失效。全命中或全未命中的筛选可以直
接复用「全部行号/空」，不必真的收集 500 万元素。

**影响范围**
仅 `src/app.rs`：`ui_result_table`、`row_filter` 的写入点（两个 metric_card
点击分支）、`JoinOutcome` 结构。lib 不变。

**预期收益**
筛选开启时消除每帧 O(N) 扫描和 ~8B/行的分配；滚动/重绘恢复流畅。

**验证方法**
手动验证：结果页点击「已匹配/未命中」卡后滚动流畅、行数正确；`cargo test`。

**风险点**
缓存键必须同时含结果版本与筛选条件，只比筛选条件会在重跑 join 后读到旧行号。
仅影响展示集合，不影响 join 语义。

## 4. JoinedRow 压缩 + 删除 row_hit

**问题现状**
`JoinedRow{left_row: usize, right_row: Option<usize>}` 实测 24B（`Option<usize>`
无 niche 优化，独占 16B）；500 万行结果占 ~120MB。`row_hit: Vec<bool>` 与
`right_row.is_some()` 逐行等价，是冗余的平行数组（另占 ~5MB）。

**原因分析**
行号用了平台宽度 `usize`，Excel 单表上限 104 万行，`u32` 足够；命中标志维护
了一份可以由引用字段直接派生的副本。

**优化措施**
- `JoinedRow` 改为 `{left_row: u32, right_row: u32}`，`u32::MAX` 作未命中哨兵，
  压到 8B/行。不要用 `as u32` 静默截断：`run_join` 入口对
  `left.rows.len()`/`right.rows.len()` 超过 `u32::MAX` 的情况直接报错（超出
  Excel 容量，实际不可能）。
- 删除 `JoinResult::row_hit` 字段；在 `JoinedTable` 上加
  `pub fn row_hit(&self, row: usize) -> bool`（`rows[row].right_row != u32::MAX`）
  保持调用方语义。`matched_rows` 计数在推入循环里顺手累计，不再事后扫描。

**影响范围**
`src/join.rs`：`JoinedRow`（pub 字段类型变化）、`JoinedTable::cell`、
`run_join` 推入处、`JoinResult`；`src/app.rs`：`run_join` 统计、
`ui_result_table` 筛选读取 `row_hit` 处、`JoinOutcome.row_hit`（可保留
`Vec<bool>` 快照或改为按 `JoinedTable::row_hit` 查询）；`join.rs` 内测试断言、
`scripts/join_bench.rs` 中 `result.row_hit[output]` 的读法需同步改。

**预期收益**
500 万行结果：引用数组 120MB→40MB（-67%），外加省掉 `row_hit` 的 5MB 与一次
事后全扫。join 耗时基本持平（写入字节减少，可能略快）。

**验证方法**
`join.rs` 现有单测改写断言后全过；`tests/end_to_end.rs` 不变（经 `cell()` 接
口验证语义）；用多变体基准复测 `u500k`、`high_500k_expand_reject` 的 RSS。

**风险点**
- `u32` 行号需要入口守卫，守卫失败走报错而非截断。
- `row_hit` 语义必须逐行等价：当前推入处 `Some→true`/`None→false` 一一对应，
  测试（`row_hit_marks_left_unmatched`、`duplicate_right_key_fanout` 等）覆盖
  充分，改造后由 `row_hit()` 方法重新暴露同一语义。
- 该字段是 `pub`，属于 lib 对 GUI/bench 的可见 API 变化，调用点都要改到。

## 11. 大表 drop 移出 UI 线程

**问题现状**
`apply_load_result`（src/app.rs:839）替换 `src.sheets`、`clear_sources`
（src/app.rs:1119）清空双侧、过期 `LoadMsg` 丢弃结果，都可能在 UI 线程上释
放一张大表最后的 `Arc` 引用。百万行表 = 百万级 `Vec`/`String` 堆块析构，单帧
可能停顿数十到数百 ms。

**原因分析**
`Table` 是 `Vec<Vec<CellValue>>` 加每格 `String` 的层级结构，drop 成本随堆块
数线性增长；这些 drop 目前都发生在帧处理的同步路径上。

**优化措施**
加一个 `drop_in_background` 辅助：把「可能是最后引用」的对象整体 move 进
`std::thread::spawn(move || drop(x))`。接入点：`apply_load_result` 覆盖
`sheets` 前先 `std::mem::take` 出旧值移交后台；`clear_sources` 的旧
`left/right`；世代不匹配被丢弃的 `LoadMsg`（内含整个 `Vec<SheetTable>`）；
`JoinOutcome` 替换（含 `Arc<Table>` 快照与 `JoinedTable`）。注意要移动整个
持有者而不是单独 clone 出的 `Arc`——只有最后的引用进后台线程，析构才真正发
生在后台；`thread::spawn` 失败时回退为原地 drop。

**影响范围**
仅 `src/app.rs`：上述各替换/丢弃点。lib 不变。

**预期收益**
消除换文件、重读、清空时的主线程释放停顿；不减少总释放工作量，只改变发生位
置。

**验证方法**
手动验证：反复加载/切换大文件、清空数据源，观察是否有明显帧停顿；
`cargo test`。

**风险点**
- 后台释放队列理论上会延迟回收、轻微抬高峰值内存；加载是低频操作，实际可忽
  略，不需要做通用析构框架。
- 对象必须满足 `Send`：`Table`/`SheetTable`/`JoinOutcome` 内部都是普通数据，
  满足；`Arc<Table>` 在线程间移动本身就是安全的引用计数操作。

## 9. Join 索引换用 foldhash

**问题现状**
`JoinIndex.rows`（src/join.rs:267）是 `HashMap<String, RowMatches>`，默认
SipHash13。B 表每行一次 `entry` 插入、A 表每行一次 `lookup`，百万行规模下哈
希本身占 join 耗时的可观份额。

**原因分析**
SipHash 为抗 HashDoS 设计，对本地可信输入是过剩的防护；`foldhash`（hashbrown
0.15 的默认 hasher）在短字符串键上明显更快。

**优化措施**
`Cargo.toml` 增加 `foldhash = "0.2"`（已在 Cargo.lock 间接依赖树中）；把
`JoinIndex.rows` 的类型改为
`HashMap<String, RowMatches, foldhash::fast::RandomState>`，构造从
`HashMap::new()` 改为 `HashMap::default()`。共两处签名级改动，无逻辑变化。

**影响范围**
`src/join.rs` 的 `JoinIndex`（私有类型，不改 pub API）、`Cargo.toml`、
`Cargo.lock`。

**预期收益**
多变体基准实测（fh vs base，中位数）：全部 10 个场景一致提速
-5%~-24%——u1m 归一化 -12%、bracket_100k -24%、u500k -9%、拒绝场景 -8%。内
存与分配次数不变。

**验证方法**
`cargo test`（语义不变，现有测试直接覆盖）；合入后更新
`scripts/bench-baseline/` 快照为当前代码再跑 `scripts/bench-join.sh` 留档。

**风险点**
foldhash 抗恶意碰撞能力弱于 SipHash；输入是用户本地 Excel 文件，不存在远程
DoS 面。map 迭代顺序会变，但输出顺序由 A 表行序决定，不依赖 map 迭代。

## 2（修改后方案）. 用 max_dup 上界跳过预估

**问题现状**
`expand_dup` 默认开启且 GUI 恒传 `limit=5M`，于是 `run_join`
（src/join.rs:516-528）总是先跑 `index.estimate()`：对 A 表全量做一遍
`write_key` + hash lookup；随后 probe 循环把同样的键规范化和查找再做一遍。

**原因分析**
预估存在的理由是在物化之前拿到精确的 `output_rows`，用于拒绝判定和诊断文案。
但在「不可能超限」的情形下这一遍是纯浪费——而判断是否可能超限其实不需要扫
A 表：索引构建时已产出精确的 `max_dup`（B 侧单键最大重复数）。

**优化措施**
保守上界：任一左行最多命中 `max_dup` 个右行；Left 下未命中也只占 1 行，所以
`left_rows × max(1, max_dup)`（saturating_mul）是输出行数的可靠上界，对
Left/Inner 同样成立。`bound <= limit` 时预估必然通过，直接跳过 `estimate()`
进入单遍 probe；`bound > limit` 时维持现有两遍流程，拒绝路径的耗时、内存、
精确诊断全部不变。跳过路径上 `reserve_rows` 对 Left 按
`min(left.len(), limit)` 预分配（每行至少产出一行）；`timings.estimate` 仅在
真正执行预估时填充。

与最初提议的「纯单遍融合」对比：纯融合在拒绝路径要先推入 ~limit 条引用才发
现超限，实测拒绝场景耗时 30ms→70ms、峰值 RSS 94MB→214MB；本方案用免费的上
界判断把两种路径分开了。

**影响范围**
`src/join.rs` 的 `run_join` 内部（~10 行），pub API 与返回语义不变。

**预期收益**
实测（fuse2 vs base，交错复测中位数）：expand 场景 -16%~-32%
（u500k -16%、d10_500k -24%、bracket_100k -32%、u100k -20%）；A 很小的场景
（high_500k_bracket_100，A=5k）无变化——预估遍本来就便宜。noexpand/inner 无
预估遍，不受影响。拒绝场景与 base 完全一致（30ms/94MB）。注意收益不是
「join 减半」：预估遍只占 A 侧 probe 成本，B 索引构建与输出写入不变。与 #9
叠加后 expand 场景合计 -25%~-35%。

**验证方法**
现有单测直接覆盖该路径：`estimate_saturates_and_limit_is_strict`（等于上限
放行、超过拒绝）、`estimate_matches_*` 口径、`high_500k_expand_reject` 场景
（bound=10M>5M → 仍走预估并精确拒绝）。建议新增一条测试：构造 bound≤limit
的用例确认结果与预估路径一致。基准复测 expand + reject 场景确认无回归。

**风险点**
- 上界必须保守：`max_dup` 是索引构建时的精确值；`max(1,…)` 覆盖全空 B
  （max_dup=0）时 Left 未命中行仍各占 1 行的情形；`saturating_mul` 防溢出。
- 跳过时 `timings.estimate` 为 `None`：该字段只供诊断/benchmark，app 的错误
  分支只读 `limit.estimate`（Err 内仍带精确预估），无影响。
- 若未来新增「上限内也想看精确预估」的调用方，需要重新评估跳过逻辑。

## 已评估、暂不做

- **纯单遍融合（#2 原案）**：接受路径收益与 fuse2 相同，但拒绝路径 +134%
  耗时、+120MB 临时分配；提前 break 的变体救不回（推入开销是主体）且丢失精
  确诊断。被 fuse2 严格替代。
- **索引键 String→Box<str>**：每键省 8B，但 `into_boxed_str` 在 cap>len 时
  realloc；索引 join 完即释放，不减少长期持有，性价比低。
- **Table 扁平化（Vec<CellValue>+stride）**：能省行级分配与缓存开销，但
  `pub rows` 牵动 lib/app/tests 十余处，且会让 `try_reheader_sheet` 的前缀删
  插从搬行描述符变成搬整片单元格，需要额外的偏移设计；当前没有测量表明行级
  分配是主要瓶颈。
- **vendor patch rust_xlsxwriter**：`write_string(&String)` 确实存在
  `Into<String>` clone + `Arc::from` 再复制 + `chars().count()` 全扫，但根治
  要维护一份 fork；先 profile 文本密集导出确认占比后再说。

## 落地记录（本轮）

6 项（1、2、3、4、9、11）已全部落地，下面是实际落点与方案不同的地方。

**#4 JoinedRow 压缩**：`JoinedRow` 改为两个 `u32`，新增哨兵
`UNMATCHED_RIGHT_ROW` 与 `JoinedTable::row_hit(row)`；`JoinResult::row_hit`
平行数组删除，改为在推入循环里累计 `matched_rows`（未命中数由
`out_rows - matched_rows` 派生）。u32 溢出守卫用的是入口 `assert!`：公返回类型
`JoinLimitExceeded` 只表达"超过输出上限"，装不下"源表行号超出 u32"这个语义，
而 Excel 单表上限让这条路径实际不可达。`run_join` 在 UI 线程外跑并带
`catch_unwind`，真触发时界面会看到"连接过程发生内部错误（已中止）"。

**#9 foldhash**：`JoinIndex.rows` 换成 `HashMap<String, RowMatches,
foldhash::fast::RandomState>`，`Cargo.toml` 增加 `foldhash = "0.2"`（该版本已在
依赖树中）。

**#2 上界跳过预估**：`estimate_needed = left_rows × max(1, max_dup) > limit`，
为假时跳过 `index.estimate()`。跳过路径上 `timings.estimate` 保持 `None`；
`reserve_rows` 对 Left 按 A 行数预留（上界已保证它不超过 limit）。新增单测
`upper_bound_within_limit_skips_estimate_with_same_output`，确认跳过路径与预估
路径的输出逐行一致。

**#1 Join 后台化**：新增 `JoinMsg`/`join_rx`/`join_gen`/`join_active`，帧末
`poll_join` 校验世代号后装配 `JoinOutcome`；限流诊断文案抽成
`limit_exceeded_message`，在后台线程里生成。连接期间按钮显示"正在连接…"并置灰，
结果页显示"正在后台连接两张表…"占位。一次连接尝试无论成功还是被拒都在结果页
收尾，与改造前一致。

**#3 筛选行号缓存**：缓存为 `FilterCache { result_id, filter, rows }`，键同时含
结果版本与筛选条件；`FilteredRows` 用 `All` 表示全集，全命中/全未命中的筛选不
物化行号表。`ui_result_table` 改为 `&mut self` 以便写入缓存。结果被替换/清空
的每条路径都走 `drop_result()`，缓存随之失效。

**#11 后台释放**：新增 `drop_in_background`，接入 `apply_load_result`（整本
工作簿替换、单工作表替换、过期 `LoadMsg`）、`clear_sources` 与 `drop_result`。
过期加载消息改为在 `apply_load_result` 内部整体移交，函数签名相应改为接收
`LoadMsg`。

`scripts/join_bench.rs` 读命中标志处加了 `cfg` 分支：基线快照仍是物化实现
（命中标志是 `JoinResult::row_hit` 数组），当前实现走 `JoinedTable::row_hit`。
两侧编译与运行都已验证。

### 验证情况

`cargo test` 全绿（lib 51 项，含新增 2 项；bin 13 项；`tests/end_to_end.rs` 5 项）。
基准脚本的 10 个场景用当前代码逐一跑通，输出内容仍按生成规则逐格校验；接受路径
的 `预估行` 全部为 `None`（预估确实被跳过），`high_500k_expand_reject` 仍在拒绝
路径给出精确预估 10,000,000。`cargo check --target x86_64-pc-windows-gnu` 通过；
本机无头启动冒烟到"界面已显示,启动完成"，无 panic。GUI 端到端按 AGENTS.md 约定
由用户手测。

`scripts/bench-baseline/` 快照本轮未刷新。当前快照是"行引用改造"之前的物化
实现，刷新它需要同时去掉 driver 里的 `benchmark_baseline` 分支（属于独立的
bench 基建改动）；而本轮的收益数字来自仓库外的多变体基准，用该脚本也复现不了。
建议单独一轮处理。

`scripts/bench-join.sh` 已补上 current 侧的 foldhash 依赖：`src/join.rs`
改用 foldhash 后 rustc 直编需要外部 crate，脚本先 `cargo build --release
-p foldhash` 产出 rlib，编译 current 的 lib 时显式 `--extern`、编译 driver
时经 `-L dependency` 供 rustc 定位；baseline 快照无此依赖不受影响。已用
`u100k_exact_narrow_100` 实测跑通（基线 ~35ms / 当前 ~18ms，预估行=None，
逐格校验通过）。
