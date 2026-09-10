# ExcelLookup

一个图形化的 Excel 双表连接(VLOOKUP / Join)小工具,免安装、单文件、开箱即用。

把 Excel 的 `VLOOKUP` 和 Power Query「合并查询」做成了双击即用的桌面应用:
不需要写公式,选文件、选列、点执行,结果预览无误后一键导出。

## 功能

- **连接方式**(A 恒为主表,想以 B 为主先「对调 A/B」)
  - 左连接(VLOOKUP 语义):保留主表所有行,匹配不到补空
  - 内连接(交集):只保留两表都能匹配的行
- **数据源灵活**:两个不同的 Excel 文件,或**同一个 Excel 的两个工作表**(每个数据源独立切换 Sheet)
  - **对调 A/B**:一键互换主/匹配表,匹配列随表交换
- **键匹配智能**
  - 键列任意选(可多列组合,后续版本开放多键 UI)
  - 宽松匹配:数字 `1001` 与文本 `"1001"` 视为相同,自动忽略首尾空格(可关闭)
- **结果可控**:右侧表可勾选任意多列带出,重复键自动展开多行
- **结果预览**:虚拟滚动表格,十万行也不卡;匹配/未命中统计一目了然
- **导出**:`.xlsx`(表头加粗、冻结首行、自适应列宽)
- **格式支持**:读 `.xlsx / .xls / .xlsb / .ods`,导出 `.xlsx`
- **中文界面**:Windows / Linux 均自动使用系统中文字体

## 下载

- **Linux arm64**(麒麟 / UOS / 树莓派等):从 [Releases](https://github.com/sg8010/excelookup/releases) 下载 `excelookup-linux-arm64`
- **Windows x64**:见下方「从源码构建」,或等待 Release 附件

```bash
chmod +x excelookup-linux-arm64
./excelookup-linux-arm64
```

## 使用

1. **数据源 A(主表)** → 选择文件;多 Sheet 文件可再选「工作表」
2. **数据源 B(匹配表)** → 同上(可与 A 选同一个文件的不同 Sheet)
3. 选**连接类型**、**A 键列 / B 键列**(要按哪列匹配)
4. 勾选 B 中要**带出的列**;按需开关「键宽松匹配」
5. 点 **执行连接**,下方预览结果与统计
6. 满意后点 **导出结果**,保存为 xlsx

## 从源码构建

### 环境要求

- Rust stable(2024 edition)
- Linux 桌面需系统自带 CJK 字体(Noto Sans CJK / 文泉驿等),Windows 用微软雅黑,均可自动识别

### Linux arm64 / x64

```bash
cargo build --release            # 产物: target/release/excelookup
```

### Windows x64(在 Linux 上交叉编译)

```bash
# 依赖: mingw-w64
sudo apt install -y gcc-mingw-w64-x86-64

./scripts/build-win.sh           # 产物: target/x86_64-pc-windows-gnu/release/excelookup.exe
```

交叉链接器配置见 `.cargo/config.toml`(使用系统 `x86_64-w64-mingw32-gcc`)。

## 测试

```bash
cargo test        # join 引擎单测 + 真实 xlsx 端到端
```

Join 性能基准使用同一驱动直接编译原始基线和当前核心库（`rustc -O`），覆盖
10 万、50 万、100 万行，唯一键、约 10% 重复、高重复、匹配率、文本长度、表宽、
数字文本归一化、括号归一化、展开开关及超限拒绝。纯计时使用系统分配器，
分配统计单独运行；每次采样独立进程。详细方法与结果见 [基准报告](docs/join-benchmark.md)。

```bash
./scripts/bench-join.sh
# 或只跑一个场景（两版各三次计时、一次分配统计）：
./scripts/bench-join.sh u100k_exact_narrow_100
```
两版分别来自 `scripts/bench-baseline/` 本轮基线快照和当前 `src/`；脚本不联网、
不切换工作树，产物和原始日志写入运行时打印的临时目录。上一轮以
`f6bd822` 为基线的完整采样仍见基准报告，基准不作为产品二进制参与默认构建。

## 技术栈

| 层 | 选型 |
|---|---|
| GUI | egui / eframe 0.36(即时模式,自带虚拟化表格) |
| 读取 Excel | calamine(xlsx/xls/xlsb/ods) |
| 写 Excel | rust_xlsxwriter |
| 文件对话框 | rfd |
| 核心逻辑 | 独立 lib(`excelookup_lib`),与 GUI 解耦、可单测 |

代码结构:

```
src/
├── main.rs        # 二进制入口(GUI)
├── app.rs         # egui 界面:数据源卡片 / 连接配置 / 结果表
├── lib.rs         # 库入口
├── model.rs       # 数据模型 CellValue / Table
├── read_xlsx.rs   # 工作簿读取(多 Sheet)
├── join.rs        # join 引擎(Left/Inner、复合键、宽松匹配)
└── export.rs      # 导出 xlsx
scripts/join_bench.rs # 两版共用的性能基准驱动
scripts/bench-baseline/ # 本轮修改前的核心源码快照
build.rs           # Windows 目标时把 assets/icon.ico 嵌入 exe(交叉编译也生效)
assets/            # icon.png(窗口/任务栏图标)、icon.ico(exe 图标资源)
tests/end_to_end.rs  # 真实文件端到端测试
```

## 发布流程

打 tag 即自动在 GitHub Actions 的 arm64 runner 上测试、编译并发布 Release:

```bash
git tag v0.2.0
git push origin v0.2.0
```

## 路线图

- 多列复合键 UI
- CSV 支持
- 未匹配行高亮 / 反查
- Windows x64 Release 附件

## License

MIT(待补 LICENSE 文件)
