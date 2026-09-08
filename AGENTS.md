# AGENTS.md — 给在此仓库工作的 Agent 的指南

## 项目概述

ExcelLookup:Rust 编写的 Excel 双表连接(VLOOKUP/Join)GUI 小工具。
- GUI:egui/eframe 0.36(中文界面)
- Excel 读:calamine(0.36);写:rust_xlsxwriter(0.99)
- 核心 join 逻辑是独立 lib `excelookup_lib`(src/lib.rs),与 GUI 解耦
- 交付目标:Linux arm64(CI)与 Windows x64(交叉编译),单文件免安装

## 常用命令

```bash
# 构建/测试(debug / release)
cargo build
cargo test                     # 15 个测试:join 单测 + 真实 xlsx 端到端
cargo build --release          # Linux 产物: target/release/excelookup

# Windows x64 交叉编译(需 mingw-w64)
./scripts/build-win.sh         # 产物: target/x86_64-pc-windows-gnu/release/excelookup.exe

# 冒烟运行 Linux GUI(无头/远程环境需软件渲染 + Wayland 转发)
XDG_RUNTIME_DIR=/mnt/wslg/runtime-dir WAYLAND_DISPLAY=wayland-0 \
LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe ./target/debug/excelookup
```

## 架构速览

| 文件 | 职责 | 注意 |
|---|---|---|
| `src/main.rs` | bin 入口,eframe 启动 | 别放业务逻辑 |
| `src/app.rs` | egui UI:Source 卡片、连接配置、结果表 | egui 0.36 借用规则:UI 回调内避免持有 `&mut self` + 遍历字段,先 clone 数据再渲染 |
| `src/model.rs` | `CellValue`(Number/Text/Empty)+ `Table` | 核心数据结构 |
| `src/read_xlsx.rs` | calamine → 多 Sheet `Table` | 首个非空行作表头,空名/重名自动修正 |
| `src/join.rs` | join 引擎:Left(VLOOKUP)/Inner/Right/Full | 哈希索引 O(n+m);复合键;`KeyMode::{Exact,Normalize}` |
| `src/export.rs` | 写 xlsx(表头样式/冻结/列宽) | — |
| `tests/end_to_end.rs` | 真实 xlsx 端到端 | 需在临时目录造数,勿提交测试文件 |

### 重要约定
- **所有用户可见文案必须是中文**(产品名 ExcelLookup、A/B 标记、VLOOKUP 除外)
- join/read/export 是纯逻辑,必须保持与 GUI 无关,便于测试与未来 CLI
- 新功能先加 lib 层 + 单测,再接 UI

## 依赖版本坑位(重要)

- **eframe/egui 0.36 API 大改**:`App` trait 的入口是 `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)`,不是旧版 `update(&Context)`;`CentralPanel::show(ui, …)` 不再有 `show_inside`。改 UI 前先查本地源码:
  `~/.cargo/registry/src/index.crates.io-*/eframe-0.36.1/src/epi.rs`
- `egui::FontData` 需包 `Arc<FontData>`,字体默认不含 CJK —— 见 `app.rs` 的 `install_cjk_font()`(运行时从系统加载,勿内嵌大字体)
- 表格用 `egui_extras::TableBuilder` + `body.rows(...)` 虚拟滚动,勿手写 for 循环
- 文件对话框 rfd 是阻塞的,勿在 UI 闭包内直接调用 → 用 `pending_open/pending_save` 标志延迟到帧末处理(见 app.rs)

## 交叉编译经验(Windows x64)

开发环境是 **WSL2 Ubuntu + 宿主机 Windows**。打通路径(勿走弯路):

1. `rustup target add x86_64-pc-windows-gnu`
2. `sudo apt install gcc-mingw-w64-x86-64`(Linux 原生 mingw,**不要**用 MSYS2 的 dlltool.exe——PE 进程不认 Linux 路径会挂死)
3. `.cargo/config.toml` 已配 `linker = "x86_64-w64-mingw32-gcc"`
4. 验证:编译后 `file` 应为 `PE32+ executable (GUI) x86-64`
5. WSL2 里可直接 `./xxx.exe`(经 interop 弹到 Windows 桌面)或拷到 `/mnt/c/` 双击冒烟;截屏验证用宿主机 PowerShell

**踩过的坑(勿重试)**:
- `x86_64-pc-windows-gnullvm` target + zig → import lib 风格不兼容,放弃
- zig 作 linker 传 `-nolibc` 冲突、MSYS2 dlltool 跨系统路径挂死 → 一律用 apt 的 Linux 原生 mingw

## Git / 发布

- 远端:`origin` = `https://github.com/sg8010/excelookup`(public)
- 主分支 `main`;历史提交用中文 message
- **CI 只在打 tag 时跑**(workflow `.github/workflows/linux-arm64.yml`,`on.push.tags: ['v*']`),main push 不触发
- CI 内容:ubuntu-24.04-arm 原生 runner → `cargo test --release` → `cargo build --release` → 上传 artifact + 打 tag 时自动建 GitHub Release(`excelookup-linux-arm64`)
- 发布新版本:`git tag vX.Y.Z && git push origin vX.Y.Z`,Release 自动出现

## 环境事实(本机 WSL2)

- 构建工具在 `~/.cargo/bin`、`~/.local/bin`,rustup 在 `~/.rustup`
- 需要代理访问外网时:HTTP(S)_PROXY 指向 `http://127.0.0.1:10808`(勿写入仓库文件)
- WSLg 跑 GUI 需软件渲染 env(WSLg 默认 EGL 会崩),见上文冒烟命令
- 本机无 cargo 时先 `export PATH="$HOME/.cargo/bin:$PATH"`
