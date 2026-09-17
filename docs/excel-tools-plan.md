# Excel 工具集改造计划

本文档是把 ExcelLookup 改造成 Cargo workspace、并在此基础上扩展出合并、拆分、脱敏等
工具的完整计划。它合并了此前三份文档（`excel-tools-workspace-plan.md`、
`excel-tools-roadmap.md`、`excel-tools-workspace-review.md`），那三份已删除，本文档是
唯一权威来源：方向说明、分阶段路线、代码核查结论与评审修正都在这里。

**当前状态**：阶段 0 与阶段 1 尚未实施；阶段 2 及以后待排期。已定案的是方向、边界和
阶段 0/1 的做法。

## 1. 方向与边界

### 1.1 已定案的结论

- **不复制项目分别改**。文件对话框、字体、启动诊断、vendor 补丁、CI 的修复只应存在
  一份，复制多份后每个修复都要同步多次。
- **改成 Cargo workspace，当前项目原地迁入**作为第一个成员，不推倒重写。
- **新程序复用控件与任务设施，各自维护自己的状态和页面**。`ExcelLookupApp` 围绕 A/B
  两表设计，不作为基类。
- **每个业务功能先写成不依赖 GUI 的 lib，并配真实 xlsx 测试，再接界面**。
- **第二个应用选「合并工作簿」**，它最简单，也最能检验共享层的接口是否够用。
- **渐进拆分**。初期只拆两个共享 crate，业务 crate 与统一的任务接口等第三个工具出现
  后再抽，避免过早抽象。
- 最合适的方向是保留本项目作为成熟参考和第一个业务模块，以 workspace 共享基础设施；
  不要复制项目重新开发，也暂时不要构建复杂的插件系统。

### 1.2 表格数据模式与工作簿保真模式

当前处理链路是「Excel → 单元格值 → `Table` → 新建 Excel」。`CellValue` 只有
`Number` / `Text` / `Empty`；读取时日期和布尔转成文本、错误单元格转成空值，公式、样式、
合并单元格、图片、隐藏状态都不进入 `Table`。因此要把功能分成两类，并在产品定义时就标明
每个功能属于哪一类：

| 模式 | 保证什么 | 引擎 | 适用功能 |
| --- | --- | --- | --- |
| 表格数据模式 | 行、列、值（含日期、布尔的类型）。输出是重新生成的规范化工作簿 | calamine + rust_xlsxwriter（现有） | Join；按行合并；按条件拆分；对指定列脱敏后生成新表 |
| 工作簿保真模式 | 尽量保留公式、样式、合并单元格、图片、隐藏表 | 需另选：直接操作 OOXML 包，或具备完整读写能力的库 | 每个 Sheet 原样放入新工作簿；在原文件中替换敏感信息并保留其余内容 |

**本计划只覆盖表格数据模式**。保真模式的功能列入需求但不排期，等有明确用户诉求时再单独
选型。切勿把 `Table` 扩成完整的 Excel 文档模型。

### 1.3 多个单用途程序还是一个工具箱

两种打包方式各有代价。多个单用途程序发版互不影响；一个工具箱只需交一次「部署税」——
deb 的 dlopen 依赖声明、launcher 脚本、Win7 导入表审计、启动日志目录都是按二进制数量
重复的。

现在不必决定，但共享层的设计要让两条路都走得通：每个应用的根是一个实现了小接口的
struct（产品标识 + 每帧绘制 + 帧末轮询），`main.rs` 只负责把它交给 `excel-desktop::run()`。
这样工具箱就是一个同时装载多个该接口实现、多一个首页的普通应用。建议在第三个工具接近
完成时，按当时的用户反馈和维护节奏做决定（见阶段 4 末）。

## 2. 现状盘点

### 2.1 可复用部分

仓库当前约 9191 行 Rust（src 8273 + tests 918），单 crate，lib 与 bin 共享一个 package。

| 现有模块 | 行数 | 复用程度 | 去向与说明 |
| --- | --- | ---: | --- |
| `src/model.rs` | 118 | 高 | `excel-core`。只依赖标准库，零 GUI 引用；扩展是追加变体，不是重写 |
| `src/join.rs` | 1174 | 业务专用 | `excel-op-join`。键归一化（`join.rs:344-458`）是纯函数，抽到 `excel-core::normalize` |
| `src/read_xlsx/` | 1027 | 高 | `excel-io`。整表与流式两条路径目前是两份语义等价的实现，靠差分测试守护 |
| `src/export.rs` | 286 | 高（需拆分） | 通用部分进 `excel-io`，Join 特化部分进 `excel-op-join`，见阶段 1 |
| `src/filebrowser.rs` | 460 | 高 | `excel-io`。纯 std。内含 Unix/Linux 桌面假设（`HOME`、`XDG_CONFIG_HOME`、根目录名、中文类型标签），跨平台需小改 |
| `src/file_dialog.rs` | 881 | 高 | `excel-desktop`。Linux 内置对话框，自包含（含自己的色板），可原样搬 |
| `src/startup.rs` | 852 | 高 | `excel-desktop`。零 egui 代码依赖；约 35% 是 Linux 专属；产品名、环境变量、日志路径需参数化 |
| `src/app/theme.rs` | 297 | 高（且需重构） | 见阶段 3。目前是 `impl ExcelLookupApp` 的 30 个关联函数，被 5 个文件以 `Self::` 引用 113 次 |
| `install_cjk_font()` | 49 | 高 | `excel-desktop`。`app.rs:655-704` |
| `open_export_location()` | 94 | 高 | `excel-desktop`。`app.rs:559-653`，不含 egui |
| `src/app/workers.rs` | 889 | 中 | load / join / export 三份后台骨架完全手写重复，无任何 trait 抽象；阶段 3 重写成 `TaskRunner` |
| `src/app/step_config.rs`、`src/app/step_sources.rs` | 255 + 388 | 低 | 留在 `apps/excelookup`，阶段 3 改用 `SourcePicker` |
| `src/app/step_result.rs` | 548 | 中 | 虚拟表格部分阶段 3 抽成 `TablePreview` |
| CI、交叉编译、安装包配置 | — | 高 | 做成按应用名参数化的模板 |

### 2.2 不复用整个 `ExcelLookupApp`

当前应用状态明显围绕「两张表 A/B」设计：Source 固定为左右两侧（`load_gen: [u64; 2]`、
`load_active: [bool; 2]` 把「两侧」写死在类型里）；`JoinOutcome` 固定引用左右表；三步流程
固定为数据源、连接配置、连接结果；worker 与 Join、导出结果状态交织在一起。

合并、拆分、脱敏的输入数量和流程都不一样。应复用控件与后台任务设施，而不是强行让所有
程序继承同一个应用状态。建议抽出的 GUI 组件：

- `SourcePicker`：文件、Sheet、列名行选择，支持单个与多个文件。
- `TablePreview`：通用虚拟滚动预览，接受任何提供 `headers / row_count / cell` 的数据源。
- `TaskRunner`：后台执行、进度、取消和过期结果处理。
- `ProgressPanel`：统一进度展示。
- `FileDialog`。
- `AppIdentity`：产品名、副标题、版本、图标、日志目录、默认输出名。

每个程序仍然维护自己的状态和页面。

### 2.3 值得原样保留的设计

lib 层不依赖 GUI（当前六个纯逻辑文件对 egui/eframe 的引用为 0 处，成立）；大表使用
`TableBuilder::body().rows()` 虚拟滚动；后台线程计算、结果经 channel 回传、世代号丢弃
过期结果；`Arc<Table>` 共享避免大表复制；Join 结果只保存源行引用；大对象移交后台线程
析构；Linux 使用内置文件对话框；真实 xlsx 端到端测试与流式读取差分测试。

## 3. 目标结构

### 3.1 最终形态

```text
excel-tools/
├── Cargo.toml                 # [workspace] + resolver="3" + [profile.release] + [patch.crates-io]
├── vendor/                    # glutin-winit、windows-link 补丁。patch 只在 workspace 根生效
├── crates/
│   ├── excel-core/            # CellValue、Table、进度类型、取消令牌、键归一化
│   ├── excel-io/              # inspect / read_table / stream_rows、导出、文件浏览
│   ├── excel-desktop/         # 启动诊断、文件对话框、主题、字体、AppIdentity、TaskRunner、公共控件
│   ├── excel-op-join/         # 阶段 1 拆出
│   ├── excel-op-merge/
│   ├── excel-op-split/
│   └── excel-op-redact/
├── apps/
│   ├── excelookup/            # 薄二进制：状态 struct + 三个步骤页
│   ├── excelmerge/
│   └── ...
├── tests/fixtures/            # 公共测试工作簿与造数脚本（当前测试数据全部代码生成，暂不需要）
├── scripts/                   # build-win.sh、check-win7-imports.sh 改为接受包名参数
└── .github/workflows/
```

workspace 根就是仓库根，不新建 `excel-tools/` 子目录——`.cargo/config.toml`、`.gitignore`、
`scripts/` 的相对路径、README 里的 `target/` 路径、CI 的 checkout 假设都建立在这一点上。

### 3.2 依赖方向

```text
excel-core  ←  excel-io  ←  excel-desktop
     ↑            ↑
     └── excel-op-join ──┘
```

由编译器保证：core 不依赖 io，io 不依赖 desktop，三者都不依赖任何 app。`excel-desktop`
依赖 `excel-io` 是必要代价（`file_dialog` 用 `filebrowser`），不存在环。

### 3.3 只建立当时需要的部分

初期不必一次拆出这么多 crate。`excel-op-join` 在阶段 1 拆出，`excel-op-merge` 在阶段 3、
`excel-op-split` 与 `excel-op-redact` 在阶段 4、5 出现。等第三个工具出现后再确定业务 crate
之间的统一接口，避免过早抽象。

## 4. 分阶段计划

每个阶段列出目标、改动、约束和完成标准。完成标准里的 GUI 项按项目约定由人工验证。
阶段 0 与阶段 1 应各自形成可构建、可测试的提交。

### 阶段 0：原地剥离（仍是单 crate）

目标是把与 Join 无关、且迁移时必然要动的代码从 `impl ExcelLookupApp` 上摘下来，行为零
变化。这一步不动目录结构，用现有的 `cargo test` 和一次手动走完三步流程即可验证。

改动（只做这三件）：

1. `install_cjk_font()`（`app.rs:655-704`）移到 `src/app/font.rs`。依赖仅为 `egui::Context`、
   `std::fs`、`log` 和一张硬编码字体路径表，唯一调用点是 `app/workers.rs:7`。
2. `open_export_location()`（`app.rs:559-653`）移到 `src/app/platform.rs`。四个平台分支，
   不含 egui，唯一调用点是 `app/step_result.rs:381`。
3. 引入 `AppIdentity`（产品名、副标题、版本、图标、日志目录名、launcher 环境变量名、
   后台线程名、默认输出文件名）。类型最终归属 `excel-desktop`，实例由 app 构造（见阶段 1）。
   本阶段只在源码侧收编字面量，**字符串值逐字不变**：
   `startup.rs:26/158/436/439/441/461/550/597/654-656`、`shell.rs:15/53-55`、`main.rs:47`、
   `app.rs:552`，必须与 `assets/excelookup-launcher.sh:15/18/32-38/69/78` 完全一致。
   默认输出名要覆盖两个分支：`workers.rs:191`（Linux 内置对话框）与 `workers.rs:241`
   （Windows rfd）。

本阶段明确不做（推迟到阶段 3，理由见 7.1）：theme 去关联函数化、`file_dialog` 色板去重、
`WorkflowStep` 数据驱动。

约束：不改 Join 行为、不改界面、不扩展 `CellValue`。

完成标准：`cargo test` 的测试逐项与基线清单一致；手动走完加载 → 配置 → 结果 → 导出；
启动日志的路径、固定文案、版本来源、事件顺序与基线一致（允许时间、PID 不同）。

### 阶段 1：建立 workspace

目标是把仓库转成 workspace，拆出 `excel-core`、`excel-io`、`excel-desktop` 三个共享 crate、
`excel-op-join` 与 `apps/excelookup`，依赖方向由编译器保证。

```text
Cargo.toml              # [workspace] + resolver="3" + [profile.release] lto="fat" + [patch.crates-io]
crates/excel-core/      # model.rs                                       单测 2
crates/excel-io/        # read_xlsx/ + filebrowser.rs + export 通用部分
                        # + tests/stream_equivalence.rs                   单测 23 + 集成 9
crates/excel-op-join/   # join.rs + JoinedExport + write_joined_xlsx 包装
                        # + tests/end_to_end.rs                           单测 26 + 集成 5
crates/excel-desktop/   # startup.rs + file_dialog.rs + font.rs + platform.rs
                        # + AppIdentity + run()                           单测 13
apps/excelookup/        # app.rs + app/*.rs + main.rs + assets/ + debian/ + metadata.deb
vendor/                 # 原地不动
```

**导出层必须拆分**。`src/export.rs:9` 直接 `use crate::join::JoinedTable`，
`write_joined_xlsx*`（`export.rs:50-73`）与 `JoinedExport`（`export.rs:95-113`）和通用写出
循环在同一个文件里。整体搬进 `excel-io` 会让通用 IO 反向依赖 Join；若 Join 再依赖 IO 就
形成依赖环。最小拆分：

| 目标 crate | 承担内容 |
| --- | --- |
| `excel-io` | `ExportPhase`、`ExportProgress`、`ExportSource`（**改为 pub**）、`impl ExportSource for Table`、`write_xlsx*`、通用写出循环（`write_export_with_progress` **改为 pub**，供跨 crate 调用）、列宽采样与 `display_width` |
| `excel-op-join` | `JoinedExport`、`write_joined_xlsx`、`write_joined_xlsx_with_progress`；依赖 core 与 io |

原有写入循环、列宽采样、进度回调逻辑一行不改，这是接口归属调整，不涉及算法或数据结构
改写。调用方 `app/workers.rs:107` 与 `tests/end_to_end.rs:91/128` 随之改 import。

**AppIdentity 的版本与图标归属**。`startup.rs:158` 是
`option_env!("EXCELOOKUP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))`，而该变量由 app 的
构建脚本注入；`main.rs:17` 用 `CARGO_MANIFEST_DIR/assets/icon.png` 嵌入图标。这两段都不能
原样搬进 desktop——搬过去后 `option_env!` 取不到 app 注入的值、`env!("CARGO_PKG_VERSION")`
会变成 desktop 自己的包版本、`CARGO_MANIFEST_DIR` 会指向不存在 `assets/` 的目录。因此：
`AppIdentity` 类型放在 desktop，**具体实例由 app 构造**；app 求值自身版本、嵌入图标，再
传给 desktop；产品名、日志目录、launcher 环境变量和线程名保留原值；
`#![cfg_attr(..., windows_subsystem = "windows")]` 保留在 app 的二进制入口，不随 `run()`
搬走。

**启动诊断的时序必须保留**。`app.rs:499-516` 的时序是：第一帧主动 `request_repaint()`
（界面静止时 egui 不会自己重绘，否则健康运行的窗口可能一直不进入第二帧，反被看门狗当成
启动超时杀掉），进入第二帧才置 `startup_ready`、降低日志级别并写「界面已显示,启动完成」。
`excel-desktop::run(identity, app)` 用一层 wrapper 复现：转发 ui 后计数，第 1 帧请求重绘，
第 2 帧末置位并降级日志。看门狗、viewport 尺寸（1440×900 与同值最小尺寸）、图标设置也由
`run()` 承担，`apps/excelookup/src/main.rs` 缩到二三十行。app 不再持有 `startup_ready` /
`startup_frames`，`new_with_startup_marker` 收回成 `new`。**仅比较日志文字不足以验证这段
行为**。

**benchmark 需要适配新旧两套依赖布局**。`scripts/bench-join.sh:39-42` 会临时生成一个
`pub mod join; pub mod model;` 的单 crate 库，并用 `--crate-name excelookup_lib` 直编
（`bench-join.sh:51-52`）。迁移后 current 的 `join.rs` 引用的是 `excel_core`，而旧 baseline
快照仍用 `crate::model`。仅改源码路径、crate 名和驱动 import 不能修好工具链。做法：baseline
一侧保持原模块布局不动；current 一侧生成兼容包装
`pub mod join; pub use excel_core as model;`，并额外把 `excel-core` 编成 rlib 用 `--extern`
传入，使同一个驱动两侧都能编译。两侧继续使用同一驱动、同一场景、相同 `-O` 与统计口径。

**其余配套改动**：

- 根清单：`resolver = "3"`、`lto = "fat"`、两个 `[patch.crates-io]`、`vendor/` 相对根。
  `[profile.release]` 与 `[patch.crates-io]` 写在成员里会被忽略，只给一条警告。
- `.cargo/config.toml` 本身无需修改（只有三个 target 的 linker，不含路径），前提是
  workspace 根就是仓库根。Cargo.lock 也保留在根并继续提交。
- 依赖按真实使用者分配：calamine / rust_xlsxwriter / anyhow 进 io，foldhash 进 op-join，
  eframe / egui_extras / rfd 进 desktop 与 app，libc 进 desktop 的启动诊断。
  `excel-core` 只依赖标准库，不要为了统一配置给它加 calamine 等依赖。
- `build.rs` 随 app 包走：`rustc-link-arg-bin` 从 `CARGO_PKG_NAME` 推导
  （`build.rs:65` 现在硬编码 `excelookup`）；`.git/HEAD`、`.git/packed-refs`、
  `.git/refs/tags` 的 rerun 监视路径改为相对 workspace 根。
- `excelookup_lib` 这个 crate 名消失，src 内 9 处、tests 6 处、bench 2 处 `use` 需改写。
- 测试按身份映射：`stream_equivalence` → `excel-io`；`end_to_end`（依赖
  join + export + read_xlsx）→ `excel-op-join`。根 `tests/` 只放数据，虚拟根下的 `tests/`
  不会被编译。顺手给 `tests/end_to_end.rs` 的固定临时文件名加 pid 后缀——此前只修了
  `stream_equivalence`，跨进程并发跑测试仍有互踩风险。
- `scripts/build-win.sh`：包名与产物路径参数化，构建命令加 `-p excelookup`（虚拟根下不加
  会连带构建全部成员，`-Z build-std` 场景尤其贵）。`check-win7-imports.sh` 已接受产物路径
  与 target 两个参数、调用方也已传入，**接口不需要改**。
- CI：`cargo test --release --workspace`、`cargo build --release -p excelookup`、
  `cargo deb --no-build -p excelookup`。

约束：不改 Join 行为、不改界面、不扩展 `CellValue`。

完成标准见第 6 节。

### 阶段 2：扩展模型与 IO（为新工具做准备）

这是唯一允许改变 Join 可观察行为的阶段，改动集中在 `excel-core` 与 `excel-io`，要把差分
测试同步更新。

`excel-core`：

- `CellValue` 增加 `Bool(bool)` 与 `DateTime`。**日期以 Excel 序列值（f64）加类型标记
  （日期时间 / 时长、1900 或 1904 纪元）存储**，不要直接嵌入 calamine 的 `ExcelDateTime`
  ——后者含 `String` 变体，会让 `CellValue` 从 32 字节涨到 40 字节，百万行 × 十列就是
  额外 80MB，并拖慢 Join 的索引与查找。`display()` 要为日期给出稳定的中文环境常用格式；
  导出时写成 Excel 日期而不是文本。此变更会影响 Join 对日期列的键匹配与显示，需要在
  `stream_equivalence` 里明确新的期望。
- 增加 `CancelToken`（`Arc<AtomicBool>` 的薄封装）与统一的 `Progress` 类型，现有
  `ExportProgress` 归入其中。
- 把 `join.rs:344-458` 的键归一化（数字与文本互认、trim、全角半角括号折叠）抽到
  `excel-core::normalize`。拆分按列分组、脱敏做稳定映射时要回答同样的「两个值算不算同一个」
  问题。

`excel-io`：

- 读取分三个入口：`inspect_workbook(path)` 只返回 Sheet 名、已用区域大小与首行预览；
  `read_table(...)` 即现有整表加载，供 Join 使用；`stream_rows(path, sheet, opts, |row| ...)`
  逐行回调，基于现有 `read_xlsx/stream.rs` 暴露，支持取消。注意流式入口目前是 `pub(crate)`，
  这是新增对外接口，不是"暴露现成 API"。
- 导出扩展为：单工作簿多 Sheet；一次任务写多个工作簿；通用行数据源（在现有
  `ExportSource` 基础上加逐行推送的 writer）；写入先到同目录临时文件，成功后原子替换目标；
  进度回调中检查取消。

**开工前先跑一遍 `scripts/bench-join.sh` 记录基线**（枚举变大会影响 `Table` 的内存占用与
Join 索引速度，改完要对比）。但只在迁移前跑一次不足以证明阶段 2 仍有可用的测量入口——
阶段 1 已把 bench 工具链按新布局修好，阶段 2 开工前要再跑一次确认可用且结果可比较。

完成标准：新增变体的读写单测；`stream_equivalence` 更新后通过；Join 端到端测试在日期列上
的新期望写进测试而非默认通过；手动确认 Join 结果预览里日期列显示合理、导出后 Excel 识别
为日期。

### 阶段 3：第二个应用「合并工作簿」

先写 `excel-op-merge` 的纯逻辑，再做 GUI。GUI 开发过程中，只把这个应用确实需要的控件抽进
`excel-desktop`。

业务模块至少支持两种策略：相同列严格合并（列名与顺序必须一致，否则报错并指出是哪个文件
哪个 Sheet）；按列名取并集合并（缺列补空，可选增加「来源文件」「来源 Sheet」列）。第三种
「每个 Sheet 原样放入」属于保真模式，不在本阶段。边界条件：各文件列名行位置不同、空
Sheet、重复列名、单文件超过 Excel 行上限时分卷。

本阶段预期抽出的公共控件（同时回头收编 Join 侧同名代码）：

- `SourcePicker`：选文件、选 Sheet、选列名行，支持单个和多个文件。Join 页里 A/B 两侧的
  对应代码迁到它上面。
- `TablePreview`：`TableBuilder` 虚拟滚动的通用预览。
- `TaskRunner`：后台执行、进度、取消、世代号丢弃过期结果。Join 的 load / join / export
  三处（`app/workers.rs`，三份手写骨架）迁到它上面。
- `ProgressPanel`：统一的进度与完成/失败展示。

同时，theme 的去关联函数化与 `file_dialog` 的色板去重在这一阶段做（见 7.1）：
`theme.rs` 的 30 个关联函数改成自由函数或 `Theme` 结构（涉及 113 处 `Self::` 引用）；
`file_dialog.rs:19-55` 那份与 theme 逐字节相同的色板改为引用同一来源，并给 theme 补
`danger()` 访问器（当前只是 `theme.rs:241` 的内联值）。注意 `file_dialog.rs:810-829` 的
按钮（高 34px）与 theme 的按钮（高 36/42px）**度量本就不同**，统一色板时不要顺势合并
控件，否则视觉会变。

完成标准：`excel-op-merge` 单测覆盖上述边界；合并 GUI 能选多文件、预览、导出、取消；
`apps/excelmerge` 有自己的 deb 元数据、图标、launcher，CI 的 app 列表增加它；两个 Windows
产物都能冒烟。

### 阶段 4：第三个应用「按筛选项拆分」与业务 crate 拆分

第三个工具出现时，确定统一的「操作」接口：输入描述、参数校验、执行（接受进度与取消）、
输出描述。三个业务 crate 都实现它，这样工具箱形态若要落地，首页只需枚举这些实现。
（`excel-op-join` 已在阶段 1 拆出，本阶段只是让它与新建的两个业务 crate 一起实现同一接口。）

`excel-op-split` 的核心职责：按一列或多列分组；空值分组的命名；文件名非法字符与长度
处理；同名输出的处理策略；单个输出超过 Excel 行上限时继续分卷；输出文件数上限与超限
提示。逐行路由到多个输出文件，依赖阶段 2 的 `stream_rows` 与多工作簿写出。

本阶段末做出 1.3 的决定（多程序还是工具箱）。

### 阶段 5：第四个应用「敏感信息替换」

规则做成纯逻辑，可组合：固定替换；保留前后若干字符；正则替换；稳定映射（同一原值始终
得到同一替换值，映射表可导出）；哈希与带盐哈希。

安全约束写进代码而不是文档：日志、错误信息、预览缓存不得包含完整原值；默认输出到新文件，
不提供覆盖源文件的选项；预览里对敏感列默认遮罩。

原地替换并保留样式属于保真模式，不在本阶段。

### 阶段 6（可选）：工具箱入口

若 1.3 决定合并为一个二进制，新增 `apps/excel-toolbox`，装载各 `excel-op-*` 与对应的页面，
增加一个首页。共享层无需改动，这正是阶段 2 至 4 设计约束的目的。

## 5. 已知的坑

**workspace 迁移本身**

- `[patch.crates-io]` 只在 workspace 根 `Cargo.toml` 生效，成员里写了会被忽略且只给一条
  警告。迁移第一步就把它和 `vendor/` 放对位置。
- 虚拟 workspace 必须显式写 `resolver = "3"`，否则退回 resolver 1 并改变 feature 统一
  行为。本项目有大量 target 区隔依赖（rfd、libc）与多处 feature 裁剪，这一点不能漏。
- 「不上提 `[workspace.dependencies]` 就能避免 feature 并集」这个理由不成立：同一构建
  依赖图中，各成员分别声明依赖也可能发生 feature 合并。分散声明与集中声明都能正确实现，
  验收重点是各依赖边保留 `default-features = false` 与当前 feature 集合，并核对迁移前后
  的实际依赖树（`cargo tree -e features`）。
- `build.rs` 的 `cargo:rustc-link-arg-bin=<name>` 必须与二进制名一致，每个 app 各自的
  `build.rs` 从 `CARGO_PKG_NAME` 取名，或抽一个 `build-support` 辅助 crate 供各 app 调用。
- `build.rs` 的 `.git/HEAD` 等 rerun 监视路径按包根解析，迁到 `apps/excelookup` 后静默
  失效（本地打 tag 不再触发版本重编；CI 因使用 `rerun-if-env-changed` 不受影响）。
- `startup.rs` 的日志目录、`EXCELOOKUP_LAUNCHER` 环境变量、launcher 脚本三处必须同步
  参数化，否则 deb 安装后启动器与程序写到不同的日志文件。README 的安装布局章节也要跟着
  改，是四处而不是三处。
- `cargo deb` 是按包执行的，workspace 下要显式 `-p`；`assets` 路径相对包目录，
  `target/release/<bin>` 相对 workspace 根，两者基准不同。
- CI 的 buster 容器保证 glibc 2.28，新 app 的依赖若引入更高 glibc 要求会在这里暴露，
  不要为此换基础镜像。
- `eframe` 的 feature 裁剪（`default-features = false` + 4 个 feature）要逐条保留，`rfd`
  的 `default-features = false` 同理。

**测试与验收**

- `file_dialog` 与 `startup` 的单测目前靠 bin target 运行；迁进 `excel-desktop` 后变成
  该 crate 的 lib tests。若 cfg 组合（`file_dialog` 是 Linux-only）或 target 归属处理
  不当，`cargo test` 会少跑而不报错——必须按测试名逐项核对，不能只看「全绿」。
- `end_to_end` 的固定临时文件名是残余竞态（`stream_equivalence` 已用 pid + 独立目录修好）。
- CI 的 Release job 有 `if: startsWith(github.ref, 'refs/tags/v')`（`linux-arm64.yml:117`），
  普通分支上 `workflow_dispatch` 验证不到它。
- Windows 门禁要覆盖 `x86_64-win7-windows-gnu` 与 `i686-win7-windows-gnu` 两个目标
  （`build-win.sh:13`），分别检查 x64 的 `PE32+` 与 x86 的 `PE32` 产物。Windows 桌面冒烟
  只能证明所用宿主环境上的启动情况，不能替代 Win7 导入表检查。
- 干净环境里必须先 `cargo build --release` 再 `cargo deb --no-build`，反过来会因缺少发布
  二进制而失败。
- 对比改造前后的 deb，重点看安装路径、文件类型和权限。二进制大小只记录差异，拆分 crate
  后不应把字节数完全一致作为验收条件。
- 打开文件位置有 6 个 Linux 文件管理器候选（`app.rs:602-621`），零测试覆盖，剥离时只能
  靠人工验证。

**性能**

- 阶段 2 改 `CellValue` 之前先记录 bench 基线（见阶段 2）。基准快照
  `scripts/bench-baseline/` 目前冻结在「行引用改造」之前的实现上，阶段 2 之后同一 driver
  的跨版本对比会失效，需要连同 `benchmark_baseline` 分支一起处理。
- 基准脚本的 `current` 一侧在阶段 1 迁移前就是「迁移前基线」，脚本自带的 baseline 是更早
  的源码快照，留档时要标明，避免把两次改动混为一谈。

## 6. 验收门禁

### 6.1 测试按身份映射

基线：2026-09-17 执行 `cargo test --locked --offline`，lib 51、bin 13、end_to_end 5、
stream_equivalence 9，共 78 个测试全部通过，无失败或忽略项。

迁移前后的门禁是「原有测试逐项映射、全部继续执行」，计数只用于辅助发现遗漏。要保存测试
全名清单（`cargo test -- --list`）并核对迁移后的名称与所属 target。

| 迁移后位置 | 原有测试数 |
| --- | ---: |
| `excel-core` 单测 | 2 |
| `excel-io` 单测 | 23 |
| `excel-op-join` 单测 | 26 |
| `excel-desktop` 单测 | 13 |
| `end_to_end` 集成测试 | 5 |
| `stream_equivalence` 集成测试 | 9 |
| 合计 | 78 |

bin 的 13 个实际是 `startup` 12 个 + `file_dialog` 1 个，迁移后进入 desktop 的 lib tests；
app 的 bin 变成 0 个测试不代表丢测。如果导出接口拆分导致测试归属调整，记录实际去向。

### 6.2 自动验证

1. 测试逐项映射与 6.1 对照。
2. `cargo tree -e features` 对比迁移前后，确认各依赖边 feature 集合未变。
3. `cargo build --release` → `cargo deb -p excelookup`，`dpkg -c` 对比安装路径、文件类型、
   权限；安装后启动并确认日志路径与内容与基线一致。
4. `scripts/build-win.sh`：两个架构的产物类型与 Win7 导入表审计都通过。
5. 迁移后再跑一次 `scripts/bench-join.sh`，证明工具链可用、结果可与迁移前留档比较。

### 6.3 人工验证

按项目约定，GUI 改动由人工走查（不写自动截图）：加载 → 配置 → 结果 → 导出全流程，界面
与改造前一致。启动诊断的时序（第二帧才标记启动完成）需实机确认，日志文字一致不足以覆盖
这段行为。日志对比看路径、固定文案、版本来源和关键事件顺序，允许时间、PID 等动态字段
不同。

### 6.4 CI 干跑的边界

`workflow_dispatch` 在普通分支能验证测试、构建、打包与 artifact 上传，但 Release job 会
被跳过。「构建与打包干跑通过」与「实际 tag 发布通过」要分开记录，不能把前者写成 Release
步骤已验证。

## 7. 与原方案的差异

### 7.1 推迟 theme 与 WorkflowStep（阶段 0 瘦身）

原定在阶段 0 完成的 theme 去关联函数化、`file_dialog` 色板去重与 `WorkflowStep` 数据驱动
推迟到阶段 3，理由：

- `theme.rs` 是 `impl ExcelLookupApp` 的关联函数，改自由函数要动 113 处 `Self::` 引用，
  且改完要逐色核对，属于重构而非搬运；只有一个应用时共享它没有收益。
- `file_dialog.rs` 那份重复色板让该文件自包含，可以整文件原样搬入 `excel-desktop`，零视觉
  风险。先删重复再搬反而把两个独立模块耦合起来。顺序应是先搬、后去重。
- `WorkflowStep` 数据驱动涉及 41 处引用、12 处赋值、10 处跳转，且侧栏提示是运行期状态
  函数（`shell.rs:103-127` 依赖 `sources_ready` / `join_active` / `result_ready`），不是静态
  文案表。等第二个应用的真实需求来定接口形状。

这样阶段 0/1 只剩「必须动」的部分，降低「Join 行为与界面零变化」的验证成本。

### 7.2 Join 提前到阶段 1 独立

原路线图把 `excel-op-join` 留到阶段 4。改为阶段 1 就拆出，理由：`join.rs` 本来就零 GUI
依赖、依赖齐全（只需 foldhash）；提前拆出后 `end_to_end` 测试有明确归属（它依赖
join + export + read_xlsx），`apps/excelookup` 可以保持 bin-only，结构最简单。这不是过早
抽象——它是搬家，不是发明接口；阶段 4 的统一操作接口仍等第三个工具出现再说。

### 7.3 导出接口拆分

原路线图写的是「`export.rs` 迁入 `excel-io`」，没有处理它对 `JoinedTable` 的依赖。实际做法
见阶段 1：通用部分与 trait 进 io，`JoinedExport` 与 `write_joined_xlsx*` 进 op-join。

### 7.4 端到端测试归属修正

原路线图把 `end_to_end.rs` 与 `stream_equivalence.rs` 一并映射到 `excel-io`，但前者依赖
Join，而 Join 在阶段 1~4 之间不在 io，会编译不过。改为随 `excel-op-join` 走。

### 7.5 本轮范围

本轮只做阶段 0 与阶段 1。明确不做：`CellValue` 扩展（`Bool` / `DateTime`）、取消令牌、
流式回调 `stream_rows`、多 Sheet / 多文件导出、临时文件原子替换、通用 `TaskRunner`、
theme 与 `WorkflowStep` 抽象、`excel-op-merge` 及之后的所有阶段。
