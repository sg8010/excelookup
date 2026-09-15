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
  - **列名行可选**:首行是合并单元格大标题、或有多行表头时,把「列名行」指到真正的列名那一行(其余行不参与连接);每个工作表各记自己的选择
- **键匹配智能**
  - 键列任意选(可多列组合,后续版本开放多键 UI)
  - 宽松匹配:数字 `1001` 与文本 `"1001"` 视为相同,自动忽略首尾空格(可关闭)
- **结果可控**:右侧表可勾选任意多列带出,重复键自动展开多行
- **结果预览**:虚拟滚动表格,十万行也不卡;匹配/未命中统计一目了然
- **大结果低内存**:连接结果只保存 A/B 源行引用,预览和导出时按需读取,不复制整张结果表
- **导出**:`.xlsx`(表头加粗、冻结首行、自适应列宽)
- **格式支持**:读 `.xlsx / .xls / .xlsb / .ods`,导出 `.xlsx`
- **中文界面**:Windows / Linux 均自动使用系统中文字体
- **Linux 不依赖外部组件**:文件选择用内置对话框(egui 自绘),不需要 GTK / XDG Portal / zenity 这些精简桌面上常常缺失的东西

## 下载

- **Linux arm64**(麒麟 / UOS / 树莓派等):从 [Releases](https://github.com/sg8010/excelookup/releases) 下载裸二进制或 Debian 软件包
- **Windows x64 / x86**(Windows 7 及以上):见下方「从源码构建」,或等待 Release 附件

### Debian 软件包安装

Debian 软件包按系统规范安装，并自动创建桌面菜单项：

```bash
sudo apt install ./excelookup_版本_arm64.deb
# 启动器：/usr/bin/excelookup（真正的二进制在 /usr/lib/excelookup/excelookup）
```

`/usr/bin/excelookup` 是一个启动脚本，负责把启动日志写到固定位置（见下），真正的
程序在 `/usr/lib/excelookup/excelookup`。包内已声明 X11/OpenGL 运行库依赖——这些库
由 glutin/winit 在运行时动态加载，`dpkg` 从二进制的依赖表里看不到，不显式声明的话
会出现“包装得上、一启动就退出”。

如果只下载裸二进制文件，可直接运行：

```bash
chmod +x excelookup-linux-arm64
./excelookup-linux-arm64
```

直接用裸二进制时日志照常写入，只是脚本那一层负责的记录（见下）不参与。

### 启动失败排查

程序启动时会检查桌面显示环境和图形运行库。若窗口无法创建,会直接提示底层错误,
并把 eframe / OpenGL 初始化日志写入：

```text
~/.cache/excelookup/startup.log
```

如果系统设置了 `XDG_STATE_HOME`,日志位于
`$XDG_STATE_HOME/excelookup/startup.log`。日志中包含程序架构、UOS/内核版本、
`DISPLAY` 等桌面环境变量以及 X11/OpenGL 动态库探测结果,反馈问题时请一并提供该文件。
时间戳是 UTC（形如 `2026-09-15T00:32:14Z`），后面跟的 `+0.012s` 是相对本次启动的
耗时，卡在哪一步、卡了多久可以直接看出来。图形初始化期间的 glutin/winit 调试细节
只记到界面显示为止——之后收回到 Info 级别，否则空闲时每帧的窗口调用会把日志写成一个
很大的文件。

排查时还需要知道这几件事：

- **`startup.log.1` 是上一次启动的日志。** 每次启动都会把上一个日志轮转成 `.1`，
  所以“失败一次、再启动一次就正常”这种情况下，失败证据仍然保留在 `.1` 里。
  偶发故障请把两个文件一起提供。
- **动态链接器报错也在日志里。** 缺少运行库、glibc 版本不够这类失败发生在程序自己
  的代码运行之前，程序来不及写日志；Debian 包的启动脚本会先把标准错误重定向进同一
  个日志文件，所以“双击了但没有任何反应”时，日志开头可能有
  `error while loading shared libraries: ...` 这样的内容。裸二进制没有这一层。
- **崩溃会留下最后一条记录。** 段错误、总线错误、非法指令、中止这些会直接杀死进程
  的信号，会在日志末尾补一行 `异常终止: 收到信号 11 (段错误)`，紧邻的上一行就是
  崩溃前最后执行的步骤。被 `SIGKILL`（例如 OOM killer）杀掉时无法捕获，日志只会
  中断在最后一条记录上。
- **看不到提示时日志里有原因。** 程序会依次尝试 zenity / kdialog / xmessage /
  notify-send / x-terminal-emulator / xdg-open 来展示错误；目标机上这些工具可能都
  没有，此时用户看不到任何提示。每次尝试的结果都会记进日志，可以用它确认用户到底
  看到过什么。

Linux 图形版需要 X11 和 OpenGL/EGL 运行库;不同 UOS 设备的显卡驱动和运行库可能不同,
所以“系统版本相同”不代表运行环境完全相同。

## 使用

1. **数据源 A(主表)** → 选择文件;多 Sheet 文件可再选「工作表」,表头不在首行时改「列名行」
2. **数据源 B(匹配表)** → 同上(可与 A 选同一个文件的不同 Sheet)
3. 选**连接类型**、**A 键列 / B 键列**(要按哪列匹配)
4. 勾选 B 中要**带出的列**;按需开关「键宽松匹配」
5. 点 **执行连接**,下方预览结果与统计
6. 满意后点 **导出结果**,保存为 xlsx

## 从源码构建

### 环境要求

- Rust stable(2024 edition)
- Linux 桌面需系统自带 CJK 字体(Noto Sans CJK / 文泉驿等),Windows 用微软雅黑,均可自动识别
- Linux 的文件对话框是内置的,不需要额外安装 GTK / XDG Desktop Portal / zenity

### Linux arm64 / x64

```bash
cargo build --release            # 产物: target/release/excelookup
```

### Windows 7+ x64/x86(在 Linux 上交叉编译)

```bash
# 依赖: mingw-w64 + rustup
sudo apt install -y \
  gcc-mingw-w64-x86-64 binutils-mingw-w64-x86-64 \
  gcc-mingw-w64-i686 binutils-mingw-w64-i686

# 脚本会自动安装 nightly 的 rust-src(Win7 target 没有预编译 std)
./scripts/build-win.sh           # 同时构建 x64 与 x86
```

产物分别为:

```text
target/x86_64-win7-windows-gnu/release/excelookup.exe
target/i686-win7-windows-gnu/release/excelookup.exe
```

该构建使用 Rust 官方 `x86_64-win7-windows-gnu` / `i686-win7-windows-gnu`
target、nightly `build-std`、release `fat LTO` 及
`rust_xlsxwriter` 的 `constant_memory` 模式，保留文本/图片剪贴板，不引用
Windows 8 的 `PathCchStripPrefix`，并在构建结束时检查两个 PE 导入表。
交叉链接器配置见 `.cargo/config.toml`(分别使用
`x86_64-w64-mingw32-gcc` 和 `i686-w64-mingw32-gcc`)。

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
| 文件对话框 | Linux:内置 egui 对话框;Windows:rfd(原生) |
| 核心逻辑 | 独立 lib(`excelookup_lib`),与 GUI 解耦、可单测 |

代码结构:

```
src/
├── main.rs        # 二进制入口(GUI)
├── app.rs         # egui 界面:数据源卡片 / 连接配置 / 结果表
├── startup.rs     # 启动诊断:启动日志、超时看门狗、崩溃留痕、错误提示
├── file_dialog.rs # 内置文件对话框(Linux;不依赖 Portal / zenity)
├── lib.rs         # 库入口
├── model.rs       # 数据模型 CellValue / Table
├── filebrowser.rs # 目录列举/排序/过滤等文件浏览纯逻辑(可单测)
├── read_xlsx.rs   # 工作簿读取(多 Sheet)
├── join.rs        # join 引擎(Left/Inner、复合键、宽松匹配)
└── export.rs      # 导出 xlsx
scripts/join_bench.rs # 两版共用的性能基准驱动
scripts/bench-baseline/ # 本轮修改前的核心源码快照
build.rs           # Windows 目标时把 assets/icon.ico 嵌入 exe(交叉编译也生效)
assets/            # icon.png / icon.ico,以及 excelookup-launcher.sh(启动器脚本)
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
