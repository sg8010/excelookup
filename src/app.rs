//! ExcelLookup 主应用界面 (egui)

mod shell;
mod step_config;
mod step_result;
mod step_sources;
mod theme;
mod workers;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Color32, CornerRadius, Shadow, Stroke};
use egui_extras::{Column, TableBuilder};

use excelookup_lib::export::{ExportPhase, ExportProgress};
use excelookup_lib::join::{
    JoinLimitExceeded, JoinSpec, JoinType, JoinedTable, KeyMode, join_with_limit,
};
use excelookup_lib::model::{CellValue, Table};
use excelookup_lib::read_xlsx::{ReadOptions, SheetTable};

#[cfg(target_os = "linux")]
use crate::file_dialog::{self, DialogAction, FileDialog};

/// 后台加载线程回主线程的消息(数据 move,不 clone)
pub(crate) struct LoadMsg {
    side: Side,
    /// 发起时的世代号:主线程据此丢弃过期(被新请求取代)的结果
    generation: u64,
    /// 文件路径(随回包带回,供主线程写入 Source.path)
    path: PathBuf,
    /// 本次读取请求的列名行(按工作表顺序;空 = 全部自动)
    header_rows: Vec<Option<usize>>,
    /// None = 读取整个工作簿;Some = 只替换这个工作表(改列名行时使用)
    sheet_idx: Option<usize>,
    /// 成功 = 各工作表;失败 = 错误文案
    result: std::result::Result<Vec<SheetTable>, String>,
}

/// 后台 join 线程回主线程的结果。
///
/// 索引构建、连接、命中统计全在后台线程完成,结果整份 move 回来;世代号只保证
/// 被换源/清空作废的旧结果不会落回,并不会中断旧线程的计算。
struct JoinMsg {
    generation: u64,
    outcome: JoinOutcome,
}

/// 后台导出线程回主线程的消息。
enum ExportMsg {
    Progress {
        generation: u64,
        progress: ExportProgress,
    },
    Finished {
        generation: u64,
        path: PathBuf,
        result: std::result::Result<(), String>,
    },
}

/// 导出状态。Join 结果视图通过 Arc 与后台线程共享,避免为导出再复制一份大表。
#[derive(Default)]
enum ExportState {
    #[default]
    Idle,
    Running(ExportProgress),
    Done(PathBuf),
}

/// GUI 持有的工作表快照。
///
/// 表格本体放在 `Arc` 中,这样 Join 结果和后台导出可以共享源数据,不必再复制
/// 一份 A/B 表;切换或重新加载工作表时只会替换这个 Arc。
#[derive(Default, Clone)]
struct LoadedSheet {
    name: String,
    table: Arc<Table>,
    preview: Vec<Vec<String>>,
    preview_cells: Vec<Vec<CellValue>>,
    preview_non_empty: Vec<bool>,
    auto_header_row: Option<usize>,
    used_header_row: Option<usize>,
    first_row_number: usize,
}

impl From<SheetTable> for LoadedSheet {
    fn from(sheet: SheetTable) -> Self {
        Self {
            name: sheet.name,
            table: Arc::new(sheet.table),
            preview: sheet.preview,
            preview_cells: sheet.preview_cells,
            preview_non_empty: sheet.preview_non_empty,
            auto_header_row: sheet.auto_header_row,
            used_header_row: sheet.used_header_row,
            first_row_number: sheet.first_row_number,
        }
    }
}

/// 一个已打开的数据源(文件 + sheets)
#[derive(Default, Clone)]
struct Source {
    path: Option<PathBuf>,
    /// 所有工作表(名称 / 数据表 / 顶部预览),顺序与工作簿一致
    sheets: Vec<LoadedSheet>,
    /// 当前 sheet 下标
    sheet_idx: usize,
    /// 各工作表的列名行选择(与 sheets 对齐;None = 自动)。
    /// 按表分开记:同一文件里各表表头结构常常不同,切表不该相互干扰。
    header_rows: Vec<Option<usize>>,
    error: Option<String>,
}

impl Source {
    fn is_loaded(&self) -> bool {
        !self.sheets.is_empty()
    }

    fn label(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn sheet_names(&self) -> Vec<String> {
        self.sheets.iter().map(|s| s.name.clone()).collect()
    }

    fn cur_sheet(&self) -> Option<&LoadedSheet> {
        self.sheets.get(self.sheet_idx)
    }

    fn cur_sheet_name(&self) -> &str {
        self.cur_sheet().map(|s| s.name.as_str()).unwrap_or("")
    }

    fn cur_table(&self) -> Option<&Table> {
        self.cur_sheet().map(|s| s.table.as_ref())
    }

    fn cur_col_count(&self) -> usize {
        self.cur_table().map(|t| t.col_count()).unwrap_or(0)
    }

    fn cur_row_count(&self) -> usize {
        self.cur_table().map(|t| t.row_count()).unwrap_or(0)
    }

    fn cur_headers(&self) -> Vec<String> {
        self.cur_table()
            .map(|t| t.headers.clone())
            .unwrap_or_default()
    }
}

pub struct ExcelLookupApp {
    left: Source,
    right: Source,
    /// 当前工作流步骤:主工作区一次只展示一个步骤。
    step: WorkflowStep,
    join_type: JoinType,
    /// None = 未选择(该侧表被替换/切换后需重新选择)
    left_key_col: Option<usize>,
    right_key_col: Option<usize>,
    right_pick_cols: Vec<usize>,
    /// UI 用的宽松匹配开关(数字/文本互认 + trim)
    normalize_keys: bool,
    /// UI 用的括号归一化开关(中文/英文括号互认)
    bracket_fold: bool,
    /// 匹配时忽略案号末尾的分支后缀,保留原值。
    case_suffix: bool,
    /// B 同键多行是否全部展开(true=展开成多行,false=只取第一条即 VLOOKUP 语义)
    expand_dup: bool,
    result: Option<JoinOutcome>,
    /// 结果表行筛选状态(点击指标卡片切换;None=全部)
    row_filter: Option<RowFilter>,
    /// 行筛选的行号缓存(按结果版本 + 筛选条件判定是否可复用)
    filter_cache: Option<FilterCache>,
    /// 结果版本号:每次产出结果递增,作为筛选缓存的失效依据
    result_seq: u64,
    /// 后台加载通道收端(每帧 poll,取到即应用)
    load_rx: Option<std::sync::mpsc::Receiver<LoadMsg>>,
    /// 发端 clone 给每次 spawn 的后台线程(单收端收两侧结果)
    load_tx: Option<std::sync::mpsc::Sender<LoadMsg>>,
    /// 每侧加载请求世代号:发起 +1,回包 gen 不匹配则丢弃(旧请求晚到)
    load_gen: [u64; 2],
    /// 每侧是否正在后台加载
    load_active: [bool; 2],
    /// 后台 join 通道收端
    join_rx: Option<std::sync::mpsc::Receiver<JoinMsg>>,
    /// join 请求世代号:换源/清空/重新连接时递增,使旧线程结果失效
    join_gen: u64,
    /// 是否有 join 正在后台运行(期间按钮置灰,避免多个大任务叠加占内存)
    join_active: bool,
    /// 后台导出通道收端
    export_rx: Option<std::sync::mpsc::Receiver<ExportMsg>>,
    /// 导出请求世代号:清空/重算结果时递增,使旧线程结果失效
    export_gen: u64,
    /// 当前导出状态
    export_state: ExportState,
    /// 打开导出文件位置失败时显示的错误
    export_location_error: Option<String>,
    /// 已点击待处理的对话框请求(帧末统一处理)
    pending_dialog: Option<DialogRequest>,
    /// 内置文件对话框(Linux;其他平台用系统原生 rfd 对话框)
    #[cfg(target_os = "linux")]
    dialog: Option<ActiveDialog>,
    /// 内置对话框上次停留的目录(下次从这里打开)
    #[cfg(target_os = "linux")]
    last_dir: Option<PathBuf>,
    /// 对调 A/B 后帧末统一处理(重置键列/输出列/结果)
    pending_swap: bool,
    /// 启动诊断用:首帧真正呈现到屏幕后才标记启动完成。
    startup_ready: Option<Arc<AtomicBool>>,
    /// 启动诊断用:已进入的帧数。第二帧开始时说明首帧已经完成呈现。
    startup_frames: u32,
}

/// 结果表行筛选(点击对应指标卡激活,再次点击取消)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowFilter {
    /// 只看命中的行
    Matched,
    /// 只看未命中的行
    Unmatched,
}

/// 筛选后要显示的行号集合。
///
/// 500 万行的结果里「全部行」是最常见的筛选结果(全部命中/全部未命中),
/// 这种情况直接按全集遍历,不物化行号表。
#[derive(Clone)]
enum FilteredRows {
    /// 就是结果表的全部行
    All,
    /// 具体行号(升序);空集时为空 Vec,不产生分配
    List(Arc<Vec<usize>>),
}

impl FilteredRows {
    fn len(&self, total: usize) -> usize {
        match self {
            FilteredRows::All => total,
            FilteredRows::List(rows) => rows.len(),
        }
    }

    /// 第 `position` 个可见行在结果表中的行号。
    fn index(&self, position: usize) -> usize {
        match self {
            FilteredRows::All => position,
            FilteredRows::List(rows) => rows[position],
        }
    }
}

/// 行筛选的行号缓存。
///
/// 缓存键必须同时含结果版本与筛选条件:只比筛选条件会在重跑 join 后读到上一份
/// 结果的行号。缓存只影响展示集合,不参与 join 语义。
struct FilterCache {
    result_id: u64,
    filter: RowFilter,
    rows: FilteredRows,
}

struct JoinOutcome {
    /// 只保存源行引用的结果视图,不拥有一份物化结果数据。
    table: Arc<JoinedTable>,
    /// 结果视图依赖的 A/B 源表快照。
    left_source: Arc<Table>,
    right_source: Arc<Table>,
    left_matched: usize,
    left_total: usize,
    right_matched_rows: usize,
    right_total: usize,
    out_rows: usize,
    /// 命中行数(结果表口径;预计算避免每帧全扫行引用)
    matched_rows: usize,
    /// 未命中行数(结果表口径)
    unmatched_rows: usize,
    /// 本次结果版本号(筛选缓存用;见 `FilterCache`)
    result_id: u64,
    /// 本次执行是否展开重复键(结果展示说明用)
    expand_dup: bool,
    err: Option<String>,
    join_type: JoinType,
}

impl JoinOutcome {
    /// 只带诊断文案的失败结果:不持有源表快照,也没有结果行。
    fn error(message: String, join_type: JoinType, result_id: u64) -> Self {
        Self {
            table: Arc::new(JoinedTable::default()),
            left_source: Arc::new(Table::default()),
            right_source: Arc::new(Table::default()),
            left_matched: 0,
            left_total: 0,
            right_matched_rows: 0,
            right_total: 0,
            out_rows: 0,
            matched_rows: 0,
            unmatched_rows: 0,
            result_id,
            expand_dup: false,
            err: Some(message),
            join_type,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
}

impl Side {
    fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }

    fn letter(self) -> &'static str {
        match self {
            Self::Left => "A",
            Self::Right => "B",
        }
    }
}

/// 待发起的文件对话框请求
///
/// 不在点击处直接弹窗:系统原生对话框(rfd)是阻塞调用,统一放到帧末处理;
/// Linux 的内置对话框也走同一条路,保证两条实现的行为一致。
#[derive(Clone, Copy)]
enum DialogRequest {
    /// 为某个数据源选择工作簿
    Open(Side),
    /// 导出结果另存为
    Save,
}

/// 正在显示的内置对话框(Linux)
#[cfg(target_os = "linux")]
struct ActiveDialog {
    request: DialogRequest,
    dialog: FileDialog,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkflowStep {
    Sources,
    Configure,
    Result,
}

impl WorkflowStep {
    fn number(self) -> usize {
        match self {
            Self::Sources => 1,
            Self::Configure => 2,
            Self::Result => 3,
        }
    }

    fn eyebrow(self) -> &'static str {
        match self {
            Self::Sources => "第一步 · 数据源",
            Self::Configure => "第二步 · 连接配置",
            Self::Result => "第三步 · 结果预览",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Sources => "先把要连接的两张表放在一起",
            Self::Configure => "选择两张表如何对齐",
            Self::Result => "检查结果，确认后导出",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Sources => "文件只在本机读取，确认工作表后进入连接配置。",
            Self::Configure => "选中匹配列，再确定需要带出的字段。",
            Self::Result => "先看命中情况，再保存为新的工作簿。",
        }
    }
}

impl Default for ExcelLookupApp {
    fn default() -> Self {
        Self {
            left: Source::default(),
            right: Source::default(),
            step: WorkflowStep::Sources,
            join_type: JoinType::Left,
            left_key_col: None,
            right_key_col: None,
            right_pick_cols: vec![],
            normalize_keys: true,
            bracket_fold: true,
            case_suffix: false,
            expand_dup: true,
            result: None,
            row_filter: None,
            filter_cache: None,
            result_seq: 0,
            load_rx: None,
            load_tx: None,
            load_gen: [0, 0],
            load_active: [false, false],
            join_rx: None,
            join_gen: 0,
            join_active: false,
            export_rx: None,
            export_gen: 0,
            export_state: ExportState::Idle,
            export_location_error: None,
            pending_dialog: None,
            #[cfg(target_os = "linux")]
            dialog: None,
            #[cfg(target_os = "linux")]
            last_dir: None,
            pending_swap: false,
            startup_ready: None,
            startup_frames: 0,
        }
    }
}

impl eframe::App for ExcelLookupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::left("workflow_sidebar")
            .exact_size(224.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Self::navy())
                    .inner_margin(egui::Margin::symmetric(16, 25)),
            )
            .show(ui, |ui| self.ui_sidebar(ui));

        egui::Panel::top("topbar")
            .exact_size(66.0)
            .frame(
                egui::Frame::new()
                    .fill(Self::white())
                    .stroke(Stroke::new(1.0, Self::line()))
                    .inner_margin(egui::Margin::symmetric(35, 0)),
            )
            .show(ui, |ui| self.ui_topbar(ui));

        egui::CentralPanel::default()
            // 在导航栏与主工作区之间保留独立的浅色留白，避免内容贴边。
            .frame(
                egui::Frame::new()
                    .fill(Self::canvas())
                    .inner_margin(egui::Margin::symmetric(16, 0)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("workspace_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        self.ui_workspace(ui);
                    });
            });

        // 帧末:poll 后台加载结果(非阻塞;可能同时有两侧/多个结果排队)
        let msgs: Vec<LoadMsg> = if let Some(rx) = &self.load_rx {
            let mut v = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                v.push(msg);
            }
            v
        } else {
            Vec::new()
        };
        for msg in msgs {
            self.apply_load_result(msg);
        }
        self.poll_join();
        self.poll_export();
        // 帧末:处理对话框(rfd 是阻塞调用,必须放在帧末;内置对话框也统一在这里画)
        self.drive_dialog(ui.ctx());
        if self.pending_swap {
            self.pending_swap = false;
            self.swap_sources();
        }

        // 进入第二帧才宣告启动完成:第一帧的绘制和呈现发生在 ui() 返回之后,若图形
        // 驱动卡在首帧的缓冲交换里,ui() 不会再被调用,watchdog 仍能按启动超时报告,
        // 而不会因为标记过早置位、日志里反而写着"已经启动完成"。
        if self.startup_ready.is_some() {
            self.startup_frames = self.startup_frames.saturating_add(1);
            if self.startup_frames >= 2 {
                if let Some(startup_ready) = self.startup_ready.take() {
                    startup_ready.store(true, Ordering::Release);
                }
                // 图形初始化已经成功,后面每帧的 winit/glutin debug 只会让日志迅速膨胀。
                log::set_max_level(log::LevelFilter::Info);
                log::info!("界面已显示,启动完成");
            } else {
                // 界面静止时 egui 不会自己重绘,必须主动要一帧,否则健康运行的窗口
                // 可能一直不进入第二帧,反倒被看门狗当成启动超时杀掉。
                ui.ctx().request_repaint();
            }
        }
    }
}

/// 计算行筛选后要显示的行号。
///
/// 筛选结果与全集/空集重合时(全部命中、全部未命中)不物化行号表——500 万行的
/// 结果里这种情况很常见,直接按全集遍历即可。
fn filter_rows(
    table: &JoinedTable,
    filter: RowFilter,
    matched_rows: usize,
    unmatched_rows: usize,
) -> FilteredRows {
    let want_hit = matches!(filter, RowFilter::Matched);
    let selected = if want_hit { matched_rows } else { unmatched_rows };
    if selected == 0 {
        return FilteredRows::List(Arc::new(Vec::new()));
    }
    if selected == table.row_count() {
        return FilteredRows::All;
    }
    let rows: Vec<usize> = (0..table.row_count())
        .filter(|&index| table.row_hit(index) == want_hit)
        .collect();
    FilteredRows::List(Arc::new(rows))
}

/// 把「可能是最后一份引用」的对象整体移交后台线程析构。
///
/// 百万行表是 `Vec<Vec<CellValue>>` 加每格 `String` 的层级结构,析构要遍历百万级
/// 堆块;若发生在 UI 线程上,换文件、重读、清空、结果替换都会带来可感知的帧停顿。
/// 注意必须 move 整个持有者而不是单独 clone 出的 `Arc`——只有最后一份引用进了线程,
/// 析构才真的发生在后台。线程创建失败时闭包在调用线程上析构,等价于原地 drop。
fn drop_in_background<T: Send + 'static>(value: T) {
    if let Err(error) = std::thread::Builder::new()
        .name("excelookup-drop".to_owned())
        .spawn(move || drop(value))
    {
        log::warn!("后台释放线程创建失败,改为当前线程释放: {error}");
    }
}

/// 调用系统文件管理器显示已导出的文件。
fn open_export_location(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("导出文件不存在: {}", path.display()));
    }

    #[cfg(target_os = "windows")]
    {
        let selection = format!("/select,{}", path.display());
        std::process::Command::new("explorer.exe")
            .arg(selection)
            .spawn()
            .map(|_| ())
            .map_err(|_| "无法打开系统文件管理器".to_owned())
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|_| "无法打开系统文件管理器".to_owned())
    }

    #[cfg(target_os = "linux")]
    {
        use std::ffi::OsString;
        use std::process::Stdio;

        fn spawn_file_manager(program: &str, args: &[OsString]) -> std::io::Result<()> {
            std::process::Command::new(program)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map(|_| ())
        }

        let file = path.as_os_str().to_os_string();
        // 这些文件管理器支持选中指定文件;按常见桌面环境依次尝试。
        let selectors = [
            (
                "nautilus",
                vec![OsString::from("--select"), file.clone()],
            ),
            (
                "dolphin",
                vec![OsString::from("--select"), file.clone()],
            ),
            ("nemo", vec![OsString::from("--select"), file.clone()]),
            (
                "pcmanfm-qt",
                vec![OsString::from("--select"), file.clone()],
            ),
            (
                "pcmanfm",
                vec![OsString::from("--select"), file.clone()],
            ),
            ("thunar", vec![file.clone()]),
        ];
        for (program, args) in selectors {
            if spawn_file_manager(program, &args).is_ok() {
                return Ok(());
            }
        }

        // 未知桌面环境的通用回退:至少打开新文件所在目录。
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .as_os_str()
            .to_os_string();
        if spawn_file_manager("xdg-open", std::slice::from_ref(&directory)).is_ok()
            || spawn_file_manager(
                "gio",
                &[OsString::from("open"), directory],
            )
            .is_ok()
        {
            return Ok(());
        }

        Err("无法找到可用的系统文件管理器".to_owned())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        Err("当前系统不支持打开文件位置".to_owned())
    }
}

/// 安装 CJK 字体(运行时从系统加载,避免二进制膨胀)
fn install_cjk_font(ctx: &egui::Context) {
    use egui::FontDefinitions;

    let mut fonts = FontDefinitions::default();
    // msyh.ttc 的第 1 个 face 是 Microsoft YaHei UI，更接近 Windows 普通桌面控件。
    // 后续字体仅在前一个文件不存在时作为整套界面的回退字体。
    let candidates: &[(&str, u32)] = if cfg!(windows) {
        &[
            ("C:\\Windows\\Fonts\\msyh.ttc", 1),
            ("C:\\Windows\\Fonts\\simhei.ttf", 0),
            ("C:\\Windows\\Fonts\\simsun.ttc", 0),
        ]
    } else {
        &[
            // NotoSansCJK-Regular.ttc 的 face 2 是简体中文(SC); face 0 是日文(JP)。
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
            ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
            ("/usr/share/fonts/wqy-microhei/wqy-microhei.ttc", 0),
        ]
    };
    let mut loaded = false;
    for &(path, face_index) in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut data = egui::FontData::from_owned(bytes);
            data.index = face_index;
            fonts.font_data.insert(
                "system_ui".to_owned(),
                std::sync::Arc::new(data),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                let family_fonts = fonts.families.entry(family).or_default();
                if cfg!(windows) {
                    // 不再让 Ubuntu-Light/Hack 与中文字体混排，避免字宽、字重和基线不一致。
                    family_fonts.insert(0, "system_ui".to_owned());
                } else {
                    family_fonts.push("system_ui".to_owned());
                }
            }
            loaded = true;
            log::info!("已加载界面字体: {path} (face_index={face_index})");
            break;
        }
    }
    if !loaded {
        log::warn!("未找到预设 CJK 字体,将使用 egui 默认字体");
    }
    ctx.set_fonts(fonts);
}
