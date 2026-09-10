# AGENTS.md

ExcelLookup:Rust + egui/eframe 0.36 的 Excel 双表 Join GUI。交付 Linux arm64(CI)与 Windows x64(交叉编译)。

## 约定(代码看不出来)

- **GUI 改动后的端到端验证由用户手动测试**,不要写程序自动截图/自动加载数据来验证 UI(徒增调试代码,绕远路)。core 逻辑(lib 层)仍需 cargo test;内置文件对话框保留一个无头渲染冒烟测试(`file_dialog.rs::tests`,只防布局代码 panic,不验证外观)
- **所有用户可见文案必须中文**(产品名、A/B、VLOOKUP 除外)
- join/read/export 是纯逻辑 lib(`excelookup_lib`),**不得依赖 GUI**;新功能先 lib+单测再接 UI
- `egui::FontData` 需包 `Arc`;egui 默认无 CJK → `app.rs::install_cjk_font()` 运行时加载系统字体,勿内嵌大字体
- eframe 0.36 的 `App` trait 入口是 `fn ui(&mut self, ui: &mut egui::Ui, …)`(旧版 `update(&Context)` 已不存在)
- **文件对话框**:Linux 用内置 egui 对话框(`src/file_dialog.rs` + lib 的 `filebrowser`),不用 rfd —— rfd 在 Linux 靠 XDG Portal,Portal 缺失会回退调 `zenity` 外部进程,精简桌面/无桌面目标机上直接没反应;Windows 仍用 rfd(原生 IFileDialog),依赖按 target 区隔(Cargo.toml)
- 对话框请求先记到 `DialogRequest`,帧末在 `drive_dialog()` 统一处理:rfd 是阻塞调用,内置对话框也走同一条路,两条实现行为一致
- 大数据表格用 `TableBuilder::body().rows()` 虚拟滚动,勿手写循环(文件列表同理)

## 交叉编译 Windows x64(勿走弯路)

开发环境 = WSL2 Ubuntu + Windows 宿主机。已配置 `.cargo/config.toml` → `linker = x86_64-w64-mingw32-gcc`(apt 的 **Linux 原生 mingw**)。

已废弃、勿重试:`x86_64-pc-windows-gnullvm` target、zig 当 linker(import lib 风格不兼容)、MSYS2 的 dlltool.exe(PE 进程不认 Linux 路径会挂死)。

验证:编译后 `file` 应为 `PE32+ (GUI) x86-64`;WSL2 里直接跑 `./xxx.exe` 会弹到 Windows 桌面可冒烟。

## 发布

- CI 只响应 `v*` tag(main push 不触发),见 `.github/workflows/linux-arm64.yml`
- 发版:`git tag vX.Y.Z && git push origin vX.Y.Z` → 自动测试+构建+建 Release
- 提交信息用中文

## 本机环境(WSL2,不进仓库)

- 工具链:`~/.cargo/bin`、`~/.local/bin`;cargo 不在 PATH 时先 `export PATH="$HOME/.cargo/bin:$PATH"`
- 外网需代理:HTTP(S)_PROXY=`http://127.0.0.1:10808`(仅环境变量,勿写入仓库)
- 无头跑 GUI:加 `XDG_RUNTIME_DIR=/mnt/wslg/runtime-dir WAYLAND_DISPLAY=wayland-0 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe`(WSLg 默认 EGL 会崩)
