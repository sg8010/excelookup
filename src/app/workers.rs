//! 应用行为层：构造、状态操作、后台任务与对话框编排（非 UI 代码）。

use super::*;

impl ExcelLookupApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_cjk_font(&cc.egui_ctx);
        Self::configure_ui_style(&cc.egui_ctx);
        Self::default()
    }

    /// 带启动完成标记的构造入口,供二进制入口的启动 watchdog 使用。
    pub fn new_with_startup_marker(
        cc: &eframe::CreationContext<'_>,
        startup_ready: Arc<AtomicBool>,
    ) -> Self {
        let mut app = Self::new(cc);
        app.startup_ready = Some(startup_ready);
        app
    }

    pub(crate) fn key_mode(&self) -> KeyMode {
        KeyMode {
            number_text: self.normalize_keys,
            brackets: self.bracket_fold,
            case_suffix: self.case_suffix,
        }
    }

    /// 请求选择工作簿:帧末统一弹对话框(见 DialogRequest)
    pub(crate) fn pick_and_load(&mut self, side: Side) {
        self.pending_dialog = Some(DialogRequest::Open(side));
    }

    /// 使当前导出请求失效。后台线程仍可安全地完成,但其消息不会再写回新状态。
    pub(crate) fn invalidate_export(&mut self) {
        self.export_gen = self.export_gen.wrapping_add(1);
        self.export_rx = None;
        self.export_state = ExportState::Idle;
        self.export_location_error = None;
    }

    /// 作废在途/旧的 join 请求。后台线程仍会跑完,但结果回来时世代号不匹配,
    /// 只会被丢弃(见 `poll_join`)。
    pub(crate) fn invalidate_join(&mut self) {
        self.join_gen = self.join_gen.wrapping_add(1);
        self.join_rx = None;
        self.join_active = false;
    }

    /// 丢弃当前连接结果及其展示状态(筛选条件与行号缓存一并失效)。
    ///
    /// 结果可能持有整张结果表与 A/B 源表快照的最后一份引用,析构放在 UI 线程上
    /// 会造成可感知的帧停顿,因此整体移交后台线程释放。
    pub(crate) fn drop_result(&mut self) {
        self.row_filter = None;
        self.filter_cache = None;
        if let Some(outcome) = self.result.take() {
            drop_in_background(outcome);
        }
    }

    /// 取下一个结果版本号(筛选缓存的失效依据)。
    pub(crate) fn next_result_id(&mut self) -> u64 {
        self.result_seq = self.result_seq.wrapping_add(1);
        self.result_seq
    }

    pub(crate) fn export_active(&self) -> bool {
        matches!(self.export_state, ExportState::Running(_))
    }

    /// 启动后台导出。结果视图与 A/B 源表都只通过 Arc 共享,不会因导出再复制大表。
    pub(crate) fn start_export(&mut self, path: PathBuf, ctx: egui::Context) {
        if self.export_active() {
            return;
        }
        let Some((table, left_source, right_source)) = self
            .result
            .as_ref()
            .filter(|result| result.err.is_none() && result.table.col_count() > 0)
            .map(|result| {
                (
                    Arc::clone(&result.table),
                    Arc::clone(&result.left_source),
                    Arc::clone(&result.right_source),
                )
            })
        else {
            return;
        };

        self.invalidate_export();
        let generation = self.export_gen;
        let total_rows = table.row_count();
        let (tx, rx) = std::sync::mpsc::channel();
        self.export_rx = Some(rx);
        self.export_state = ExportState::Running(ExportProgress {
            phase: ExportPhase::Writing,
            completed_rows: 0,
            total_rows,
        });
        ctx.request_repaint();

        let repaint_ctx = ctx.clone();
        std::thread::spawn(move || {
            let export_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                excelookup_lib::export::write_joined_xlsx_with_progress(
                    &table,
                    &left_source,
                    &right_source,
                    &path,
                    |progress| {
                        let _ = tx.send(ExportMsg::Progress {
                            generation,
                            progress,
                        });
                        repaint_ctx.request_repaint();
                    },
                )
            }))
            .map_err(|_| "导出过程发生内部错误（已中止）".to_owned())
            .and_then(|result| result.map_err(|e| format!("{e}")));

            let _ = tx.send(ExportMsg::Finished {
                generation,
                path,
                result: export_result,
            });
            repaint_ctx.request_repaint();
        });
    }

    /// 每帧非阻塞地收取导出进度和完成消息。
    pub(crate) fn poll_export(&mut self) {
        let messages: Vec<ExportMsg> = if let Some(rx) = &self.export_rx {
            let mut messages = Vec::new();
            while let Ok(message) = rx.try_recv() {
                messages.push(message);
            }
            messages
        } else {
            Vec::new()
        };

        for message in messages {
            match message {
                ExportMsg::Progress {
                    generation,
                    progress,
                } if generation == self.export_gen && self.export_active() => {
                    self.export_state = ExportState::Running(progress);
                }
                ExportMsg::Finished {
                    generation,
                    path,
                    result,
                } if generation == self.export_gen => {
                    self.export_rx = None;
                    match result {
                        Ok(()) => self.export_state = ExportState::Done(path),
                        Err(error) => {
                            if let Some(result) = &mut self.result {
                                result.err = Some(format!("导出失败: {error}"));
                            }
                            self.export_state = ExportState::Idle;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Linux:驱动内置文件对话框(不依赖 XDG Portal / zenity)
    #[cfg(target_os = "linux")]
    pub(crate) fn drive_dialog(&mut self, ctx: &egui::Context) {
        // 1. 新请求:建对话框(初始目录沿用上次的位置)
        if let Some(request) = self.pending_dialog.take() {
            let dir = self.last_dir.clone();
            let dialog = match request {
                DialogRequest::Open(side) => FileDialog::open(
                    "选择工作簿",
                    &format!("数据源 {}", side.letter()),
                    dir,
                    file_dialog::workbook_filters(),
                ),
                DialogRequest::Save => FileDialog::save(
                    "导出结果",
                    "保存为 Excel 工作簿",
                    dir,
                    "连接结果.xlsx",
                    file_dialog::xlsx_filters(),
                ),
            };
            self.dialog = Some(ActiveDialog { request, dialog });
        }
        // 2. 已显示的对话框:画一帧并处理结果
        let Some(active) = &mut self.dialog else { return };
        let action = active.dialog.ui(ctx);
        // 记住用户停留的目录,下次从这里打开(即便这次取消了)
        self.last_dir = Some(active.dialog.dir().to_path_buf());
        let request = active.request;
        match action {
            DialogAction::None => {}
            DialogAction::Cancelled => self.dialog = None,
            DialogAction::Picked(path) => {
                self.dialog = None;
                match request {
                    DialogRequest::Open(side) => {
                        self.step = WorkflowStep::Sources;
                        // 新文件:列名行全部重新自动
                        self.start_load(side, path, Vec::new(), ctx.clone());
                    }
                    DialogRequest::Save => self.start_export(path, ctx.clone()),
                }
            }
        }
    }

    /// 其他平台:系统原生对话框(rfd;阻塞调用,所以放在帧末)
    #[cfg(not(target_os = "linux"))]
    pub(crate) fn drive_dialog(&mut self, ctx: &egui::Context) {
        let Some(request) = self.pending_dialog.take() else {
            return;
        };
        match request {
            DialogRequest::Open(side) => {
                let picked = rfd::FileDialog::new()
                    .add_filter("Excel 工作簿", &["xlsx", "xls", "xlsb", "xlsm", "ods"])
                    .add_filter("所有文件", &["*"])
                    .pick_file();
                if let Some(path) = picked {
                    self.step = WorkflowStep::Sources;
                    // 新文件:列名行全部重新自动
                    self.start_load(side, path, Vec::new(), ctx.clone());
                }
            }
            DialogRequest::Save => {
                let picked = rfd::FileDialog::new()
                    .add_filter("Excel 工作簿", &["xlsx"])
                    .set_file_name("连接结果.xlsx")
                    .save_file();
                if let Some(path) = picked {
                    self.start_export(path, ctx.clone());
                }
            }
        }
    }

    /// 启动后台加载:spawn 线程解析,主线程不阻塞;返回后界面立即可交互。
    /// `header_rows` = 各工作表的列名行(已用区域 0-based;`None` = 自动),
    /// 按工作表顺序对齐;空 Vec 表示全部自动。
    pub(crate) fn start_load(
        &mut self,
        side: Side,
        path: PathBuf,
        header_rows: Vec<Option<usize>>,
        ctx: egui::Context,
    ) {
        self.start_load_request(side, path, header_rows, None, ctx);
    }

    /// 改列名行时只重读当前工作表,不重新构造同一工作簿的其他表。
    pub(crate) fn start_sheet_reload(
        &mut self,
        side: Side,
        path: PathBuf,
        header_rows: Vec<Option<usize>>,
        sheet_idx: usize,
        ctx: egui::Context,
    ) {
        // 先释放旧连接结果:如果源表没有被其他快照引用,下面可以直接复用其行数据。
        self.invalidate_export();
        self.invalidate_join();
        self.drop_result();
        if self.try_reheader_sheet(side, sheet_idx, &header_rows) {
            // 没有后台请求,也要让可能残留的旧消息失效。
            self.load_gen[side.index()] += 1;
            self.reset_side_on_source_change(side);
            return;
        }
        self.start_load_request(side, path, header_rows, Some(sheet_idx), ctx);
    }

    /// 在顶部候选行范围内切换列名行时,直接复用当前表的行 Vec。
    ///
    /// 这种路径不需要重新打开文件,也不会同时保留新旧大表。若当前表仍被
    /// 结果视图/导出线程引用,或目标行不在顶部候选范围内,返回 false 走后台重读。
    pub(crate) fn try_reheader_sheet(
        &mut self,
        side: Side,
        sheet_idx: usize,
        header_rows: &[Option<usize>],
    ) -> bool {
        let success = {
            let src = match side {
                Side::Left => &mut self.left,
                Side::Right => &mut self.right,
            };
            let sheet_count = src.sheets.len();
            let Some(sheet) = src.sheets.get_mut(sheet_idx) else {
                return false;
            };
            let Some(current_idx) = sheet.used_header_row else {
                return false;
            };
            let requested = header_rows.get(sheet_idx).copied().flatten();
            let target_idx = match requested {
                None => match sheet.auto_header_row {
                    Some(index) => index,
                    None => return false,
                },
                Some(index) if index == current_idx => index,
                Some(index) => {
                    if index >= sheet.preview.len()
                        || index >= sheet.preview_non_empty.len()
                    {
                        return false;
                    }
                    if sheet.preview_non_empty[index] {
                        index
                    } else {
                        match sheet.auto_header_row {
                            Some(index) => index,
                            None => return false,
                        }
                    }
                }
            };

            if target_idx != current_idx {
                if current_idx >= sheet.preview_non_empty.len()
                    || target_idx >= sheet.preview_non_empty.len()
                    || current_idx >= sheet.preview_cells.len()
                    || target_idx >= sheet.preview_cells.len()
                    || !sheet.preview_non_empty[target_idx]
                {
                    return false;
                }
                let old_table = std::mem::replace(
                    &mut sheet.table,
                    Arc::new(Table::default()),
                );
                let mut table = match Arc::try_unwrap(old_table) {
                    Ok(table) => table,
                    Err(old_table) => {
                        sheet.table = old_table;
                        return false;
                    }
                };
                if target_idx > current_idx {
                    let drop_count = (current_idx + 1..=target_idx)
                        .filter(|&index| sheet.preview_non_empty[index])
                        .count();
                    if drop_count > table.rows.len() {
                        sheet.table = Arc::new(table);
                        return false;
                    }
                    table.rows.drain(..drop_count).for_each(drop);
                } else {
                    // 向前切换时,从顶部候选行缓存恢复原列名行与其后的前置数据行;
                    // 仅复制至多 PREVIEW_ROWS 行,大表正文仍然直接移动。
                    let mut prefix_rows = Vec::with_capacity(current_idx - target_idx);
                    for index in (target_idx + 1)..=current_idx {
                        if sheet.preview_non_empty[index] {
                            prefix_rows.push(sheet.preview_cells[index].clone());
                        }
                    }
                    let old_rows = std::mem::take(&mut table.rows);
                    prefix_rows.reserve(old_rows.len());
                    prefix_rows.extend(old_rows);
                    table.rows = prefix_rows;
                }
                table.headers = Self::headers_from_preview(
                    &sheet.preview[target_idx],
                    table.col_count(),
                );
                sheet.table = Arc::new(table);
                sheet.used_header_row = Some(target_idx);
            }

            let mut requested_rows = header_rows.to_vec();
            requested_rows.resize(sheet_count, None);
            src.header_rows = requested_rows;
            true
        };
        success
    }

    pub(crate) fn start_load_request(
        &mut self,
        side: Side,
        path: PathBuf,
        header_rows: Vec<Option<usize>>,
        sheet_idx: Option<usize>,
        ctx: egui::Context,
    ) {
        let idx = side.index();
        // 旧请求结果作废:世代 +1;正在跑的旧线程结果回来时 gen 不匹配会被丢弃
        self.load_gen[idx] += 1;
        let generation = self.load_gen[idx];
        self.load_active[idx] = true;
        // 新请求会使旧的连接结果立即失效,先释放可能很大的结果表,避免与新表
        // 一起存活到后台加载完成。
        self.invalidate_export();
        self.invalidate_join();
        self.drop_result();
        // 新加载开始:清掉旧错误(加载成功/失败后再按结果设置)
        match side {
            Side::Left => self.left.error = None,
            Side::Right => self.right.error = None,
        }

        // 确保通道存在(首次创建)
        if self.load_rx.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<LoadMsg>();
            self.load_rx = Some(rx);
            self.load_tx = Some(tx);
        }
        let tx = self.load_tx.as_ref().expect("load_tx 已初始化").clone();
        // 只重读当前表时,先在主线程记住名称;后台线程无需访问 GUI 状态。
        let sheet_name = sheet_idx.and_then(|sheet_idx| match side {
            Side::Left => self.left.sheets.get(sheet_idx),
            Side::Right => self.right.sheets.get(sheet_idx),
        })
        .map(|sheet| sheet.name.clone());
        // 立即刷新一次 UI 显示"加载中…"(否则要等下次交互才重绘)
        ctx.request_repaint();
        // 线程只依赖 lib + repaint 句柄(不碰 GUI 状态),成功后数据 move 回主线程,不 clone。
        // catch_unwind:即使解析 panic 也发回错误消息,避免 UI 永远停在"加载中"。
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| {
                match (sheet_idx, sheet_name.as_deref()) {
                    (Some(sheet_idx), Some(sheet_name)) => {
                        let requested = header_rows.get(sheet_idx).copied().flatten();
                        excelookup_lib::read_xlsx::read_sheet_opts(
                            &path,
                            sheet_name,
                            requested,
                            true,
                        )
                        .map(|sheet| vec![sheet])
                    }
                    _ => excelookup_lib::read_xlsx::read_workbook_opts(
                        &path,
                        ReadOptions {
                            header_rows: header_rows.clone(),
                            preview: true,
                        },
                    ),
                }
            })
            .map_err(|_| "读取过程发生内部错误（已中止）".to_owned())
            .and_then(|r| r.map_err(|e| format!("{e}")));
            let _ = tx.send(LoadMsg {
                side,
                generation,
                path,
                header_rows,
                sheet_idx,
                result,
            });
            // 唤醒主线程处理结果(后台完成时 UI 可能空闲无重绘)
            ctx.request_repaint();
        });
    }

    /// 应用后台加载结果(主线程,帧末 poll 到后调用)。
    /// 世代不匹配 = 已有更新的加载请求,整份消息(可能装着整本工作簿)移交后台丢弃。
    pub(crate) fn apply_load_result(&mut self, msg: LoadMsg) {
        if msg.generation != self.load_gen[msg.side.index()] {
            drop_in_background(msg);
            return;
        }
        let LoadMsg {
            side,
            path,
            header_rows,
            sheet_idx,
            result,
            ..
        } = msg;
        self.load_active[side.index()] = false;
        // 计算需要哪些状态,借用 src 的代码全放在这个作用域里
        let mut reset = false;
        {
            let src = match side {
                Side::Left => &mut self.left,
                Side::Right => &mut self.right,
            };
            src.error = None;
            match result {
                Ok(sheets) => {
                    if let Some(sheet_idx) = sheet_idx {
                        // 改列名行的请求只返回一个表,其余已加载表保持原 Arc 不动。
                        let Some(sheet) = sheets.into_iter().next() else {
                            src.error = Some("读取工作表后没有数据".into());
                            return;
                        };
                        let same_workbook = src.path.as_deref() == Some(path.as_path());
                        if !same_workbook || sheet_idx >= src.sheets.len() {
                            src.error = Some("工作簿状态已变化，请重新选择工作簿".into());
                            return;
                        }
                        let sheet_count = src.sheets.len();
                        // 先取旧选择再覆盖,用于判断列含义是否变了
                        let mut prev_header_rows = std::mem::take(&mut src.header_rows);
                        let mut requested = header_rows;
                        requested.resize(sheet_count, None);
                        prev_header_rows.resize(sheet_count, None);
                        reset = prev_header_rows != requested;
                        src.header_rows = requested;
                        // 旧工作表可能持有大表的最后一份引用,移交后台释放。
                        drop_in_background(std::mem::replace(
                            &mut src.sheets[sheet_idx],
                            LoadedSheet::from(sheet),
                        ));
                    } else {
                        if sheets.is_empty() {
                            src.error = Some("工作簿中无工作表".into());
                            return;
                        }
                        let same_workbook = src.path.as_deref() == Some(path.as_path());
                        let prev_sheet_idx = src.sheet_idx;
                        // 先取旧选择再覆盖,用于判断列含义是否变了
                        let mut prev_header_rows = std::mem::take(&mut src.header_rows);
                        let mut requested = header_rows;
                        // 按新工作表数对齐(缺项 = 自动)
                        requested.resize(sheets.len(), None);
                        prev_header_rows.resize(sheets.len(), None);
                        // 同一文件 + 各表列名行选择未变 → 列含义不变,保留键列/输出列
                        reset = !same_workbook || prev_header_rows != requested;
                        src.path = Some(path);
                        // 整本旧工作簿可能持有大表的最后一份引用(百万行的
                        // Vec/String 析构要遍历百万级堆块),移交后台释放。
                        drop_in_background(std::mem::replace(
                            &mut src.sheets,
                            sheets.into_iter().map(LoadedSheet::from).collect(),
                        ));
                        src.header_rows = requested;
                        if same_workbook && prev_sheet_idx < src.sheets.len() {
                            // 同文件重读(改列名行):停在原工作表
                            src.sheet_idx = prev_sheet_idx;
                        } else {
                            // 跳到第一个非空 sheet
                            src.sheet_idx = src
                                .sheets
                                .iter()
                                .position(|s| !s.table.is_empty())
                                .unwrap_or(0);
                        }
                    }
                }
                Err(e) => {
                    src.error = Some(format!("打开失败: {e}"));
                }
            }
        }
        if reset {
            // 换文件/换列名行 = 列含义已变:键列/输出列需重选
            self.reset_side_on_source_change(side);
            self.drop_result();
        }
    }

    /// 切换 sheet(切换后清结果、校正键列)
    pub(crate) fn switch_sheet(&mut self, side: Side, idx: usize) {
        let src = match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        };
        if idx >= src.sheets.len() {
            return;
        }
        src.sheet_idx = idx;
        // 该侧表已切换:键列/输出列需重新选择
        self.reset_side_on_source_change(side);
        self.drop_result();
        if self.step == WorkflowStep::Result {
            self.step = WorkflowStep::Configure;
        }
    }

    /// 某侧的表被替换/切换后调用:清空该侧键列选择(严格版:即使下标合法也不保留,避免
    /// "下标合法但列含义已变"的静默错误),并恢复该侧输出列默认值(A 全选、B 清空)。另一侧不受影响。
    pub(crate) fn reset_side_on_source_change(&mut self, side: Side) {
        self.invalidate_export();
        // 该侧表要换了:在途 join 算的是换之前的快照,结果已无意义。
        self.invalidate_join();
        match side {
            Side::Left => {
                self.left_key_col = None;
                self.left_pick_cols = None;
            }
            Side::Right => {
                self.right_key_col = None;
                self.right_pick_cols.clear();
            }
        }
    }

    /// 执行连接。索引构建、连接与命中统计全在后台线程跑,期间界面保持可交互;
    /// 完成后由 `poll_join` 在帧末装配结果。
    pub(crate) fn run_join(&mut self, ctx: egui::Context) {
        self.invalidate_export();
        self.invalidate_join();
        self.drop_result();
        if !self.left.is_loaded() || !self.right.is_loaded() {
            let result_id = self.next_result_id();
            self.result = Some(JoinOutcome::error(
                "请先加载两个数据源".into(),
                self.join_type,
                result_id,
            ));
            self.step = WorkflowStep::Configure;
            return;
        }

        // 当前 sheet 只克隆 Arc,不复制表格数据;结果视图和后台导出都会共享这两个快照,
        // join 期间即便换源/切表,计算中的数据也不会被释放。
        let Some(left_table) = self.left.cur_sheet().map(|sheet| Arc::clone(&sheet.table)) else {
            return;
        };
        let Some(right_table) = self.right.cur_sheet().map(|sheet| Arc::clone(&sheet.table)) else {
            return;
        };
        let lc = left_table.col_count();
        let rc = right_table.col_count();
        if lc == 0 || rc == 0 {
            let result_id = self.next_result_id();
            self.result = Some(JoinOutcome::error(
                "当前工作表无列数据".into(),
                self.join_type,
                result_id,
            ));
            self.step = WorkflowStep::Configure;
            return;
        }
        // 键列未选择或当前表无列时不允许执行
        let (Some(lk), Some(rk)) = (self.left_key_col, self.right_key_col) else {
            let result_id = self.next_result_id();
            self.result = Some(JoinOutcome::error(
                "请先在连接配置中选择 A/B 匹配列".into(),
                self.join_type,
                result_id,
            ));
            self.step = WorkflowStep::Configure;
            return;
        };
        let lk = lk.min(lc.saturating_sub(1));
        let rk = rk.min(rc.saturating_sub(1));
        let rp: Vec<usize> = self
            .right_pick_cols
            .iter()
            .copied()
            .filter(|&c| c < rc)
            .collect();

        // 原样透传 UI 的选择(含越界项),统一由 JoinSpec 侧解析:这里再过滤一遍
        // 会与 resolved_left_pick 出现两套口径。
        let spec = JoinSpec {
            join_type: self.join_type,
            left_keys: vec![lk],
            right_keys: vec![rk],
            left_pick: self.left_pick_cols.clone(),
            right_pick: rp,
            key_mode: self.key_mode(),
            expand_dup: self.expand_dup,
        };
        // 判据与配置页共用 join::has_output_columns:两边各自过滤一遍会
        // 因为越界下标的处理差异出现「按钮可点、点了却报错」。
        if !spec.has_output_columns(lc, rc) {
            let result_id = self.next_result_id();
            self.result = Some(JoinOutcome::error(
                "请至少选择一个 A 或 B 输出列".into(),
                self.join_type,
                result_id,
            ));
            self.step = WorkflowStep::Configure;
            return;
        }

        // 重复键展开防爆:预估与真正 Join 共享同一个 B 索引；超限只返回诊断，
        // 不再先统计一次再重新构建索引(避免大表重复扫描与重复 key 分配)。
        const MAX_EXPAND_ROWS: usize = 5_000_000;
        let max_output_rows = self.expand_dup.then_some(MAX_EXPAND_ROWS);
        let join_type = self.join_type;
        let expand_dup = self.expand_dup;
        let left_rows = left_table.row_count();
        let right_rows = right_table.row_count();
        let result_id = self.next_result_id();
        let generation = self.join_gen;
        let (tx, rx) = std::sync::mpsc::channel();
        self.join_rx = Some(rx);
        self.join_active = true;
        // 结果页先显示"正在连接";旧结果已在上方丢弃,不会和新结果混淆。
        self.step = WorkflowStep::Result;
        ctx.request_repaint();

        let repaint_ctx = ctx.clone();
        std::thread::spawn(move || {
            // catch_unwind:join 内部 panic(如源表行数超出 u32 行号上限)也要
            // 变成可见的错误文案,不能让界面永远停在"正在连接"。
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match join_with_limit(&left_table, &right_table, &spec, max_output_rows) {
                    Ok(res) => JoinOutcome {
                        table: Arc::new(res.table),
                        left_source: left_table,
                        right_source: right_table,
                        left_matched: res.left_matched,
                        left_total: res.left_total,
                        right_matched_rows: res.right_matched_rows,
                        right_total: res.right_total,
                        out_rows: res.out_rows,
                        matched_rows: res.matched_rows,
                        unmatched_rows: res.out_rows - res.matched_rows,
                        result_id,
                        expand_dup,
                        err: None,
                        join_type,
                    },
                    Err(limit) => JoinOutcome::error(
                        Self::limit_exceeded_message(&limit, left_rows, right_rows),
                        join_type,
                        result_id,
                    ),
                }
            }))
            .unwrap_or_else(|_| {
                JoinOutcome::error(
                    "连接过程发生内部错误（已中止）".into(),
                    join_type,
                    result_id,
                )
            });

            let _ = tx.send(JoinMsg {
                generation,
                outcome,
            });
            repaint_ctx.request_repaint();
        });
    }

    /// 每帧非阻塞地收取后台 join 结果。
    pub(crate) fn poll_join(&mut self) {
        let messages: Vec<JoinMsg> = if let Some(rx) = &self.join_rx {
            let mut messages = Vec::new();
            while let Ok(message) = rx.try_recv() {
                messages.push(message);
            }
            messages
        } else {
            Vec::new()
        };

        for message in messages {
            if message.generation != self.join_gen {
                // 已作废的结果(期间换源/清空/重新连接):可能持有整张结果表与
                // 源表快照,移交后台线程释放。
                drop_in_background(message.outcome);
                continue;
            }
            self.join_rx = None;
            self.join_active = false;
            self.drop_result();
            self.result = Some(message.outcome);
            // 一次连接尝试都以结果页收尾:成功是结果表,失败是诊断卡片。用户中途
            // 离开结果页也会被带回来,和连接在 UI 线程上跑时的行为一致。
            self.step = WorkflowStep::Result;
        }
    }

    /// 重复键展开超出上限时的诊断文案:标题 + 数据诊断 + 排查步骤。
    pub(crate) fn limit_exceeded_message(limit: &JoinLimitExceeded, a_rows: usize, b_rows: usize) -> String {
        let estimate = limit.estimate;
        // 单位自适应:≥1 亿用亿,≥1 万用万,否则原样
        let fmt = |n: usize| -> String {
            if n >= 100_000_000 {
                format!("{:.1} 亿", n as f64 / 100_000_000.0)
            } else if n >= 10_000 {
                format!("{:.1} 万", n as f64 / 10_000.0)
            } else {
                n.to_string()
            }
        };

        // 诊断段落(多行;首行=标题,随后按行渲染)
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!(
            "重复键展开后结果约 {} 行,远超可处理范围,已中止。",
            fmt(estimate.output_rows)
        ));
        lines.push(String::new());
        lines.push("【数据诊断】".into());
        lines.push(format!(
            "· A 表(主表)共 {a_rows} 行;B 表(匹配表)共 {b_rows} 行。"
        ));

        // 原因定位:先看 B 键重复度,再看单键极端值
        if estimate.distinct_keys > 0 && b_rows >= 10 && b_rows / estimate.distinct_keys >= 10 {
            lines.push(format!(
                "· B 表键列几乎不唯一:{} 行只有 {} 个不同键值,单键最多重复 {} 次。",
                fmt(b_rows),
                fmt(estimate.distinct_keys),
                fmt(estimate.max_dup)
            ));
            lines.push("· 原因:匹配列很可能选成了“分类/枚举”类列(如省份、状态、类型),而非唯一编号列。".into());
        } else if estimate.max_dup > 1000 {
            lines.push(format!(
                "· B 表键列存在单键重复 {} 次的极端值(去重后共 {} 个键)。",
                fmt(estimate.max_dup),
                fmt(estimate.distinct_keys)
            ));
            lines.push("· 原因:B 表存在大量同键行,可能数据本身重复,或键列粒度过粗。".into());
        } else {
            lines.push(format!(
                "· B 表键去重后 {} 个(共 {} 行),A 表 {a_rows} 行平均每键命中多条。",
                fmt(estimate.distinct_keys),
                fmt(b_rows)
            ));
            lines.push("· 原因:A 与 B 的匹配列粒度不匹配(如明细对汇总),导致普遍一对多。".into());
        }

        lines.push(String::new());
        lines.push("【排查步骤】".into());
        lines.push("1. 返回“连接配置”,检查 A、B 两表的匹配列是否都选了编号/ID 类唯一列。".into());
        lines.push("2. 在“数据源”页确认 B 表:健康键列的去重个数应接近表行数(如 39 万行应有几十万个不同键)。".into());
        lines.push("3. 若 B 表确实同键多行(如一人多条记录),请关闭“重复键展开”开关(只取第一条,VLOOKUP 风格)。".into());
        lines.push("4. 若确认键列无误仍过大,可先对 A 表筛选/去重后再连接。".into());

        lines.join("\n")
    }

    pub(crate) fn clear_result(&mut self) {
        self.invalidate_export();
        self.invalidate_join();
        self.drop_result();
        if self.sources_ready() {
            self.step = WorkflowStep::Configure;
        }
    }

    pub(crate) fn clear_sources(&mut self) {
        self.invalidate_export();
        self.invalidate_join();
        // 两侧旧表可能各自持有大表的最后一份引用,移交后台线程释放。
        drop_in_background(std::mem::take(&mut self.left));
        drop_in_background(std::mem::take(&mut self.right));
        self.left_key_col = None;
        self.right_key_col = None;
        self.right_pick_cols.clear();
        self.left_pick_cols = None;
        self.drop_result();
        // 作废所有在途加载请求(清空后旧结果不得落回)
        self.load_gen[0] += 1;
        self.load_gen[1] += 1;
        self.load_active = [false, false];
        self.step = WorkflowStep::Sources;
    }

    /// 对调 A/B 两个数据源(文件+sheet+当前选中)。
    /// 匹配列随对调交换(新 A 沿用原 B 的键列,新 B 沿用原 A 的):整表互换后列号有效性自动
    /// 守恒,即便某侧此前未选择,交换后仍为 None。
    /// 主从关系已变,输出列恢复默认值(A 全选、B 清空)。
    pub(crate) fn swap_sources(&mut self) {
        self.invalidate_export();
        self.invalidate_join();
        std::mem::swap(&mut self.left, &mut self.right);
        std::mem::swap(&mut self.left_key_col, &mut self.right_key_col);
        self.right_pick_cols.clear();
        self.left_pick_cols = None;
        self.drop_result();
        self.step = WorkflowStep::Sources;
    }

    pub(crate) fn sources_ready(&self) -> bool {
        !self.load_active[Side::Left.index()]
            && !self.load_active[Side::Right.index()]
            && self.left.is_loaded()
            && self.right.is_loaded()
            && self.left.error.is_none()
            && self.right.error.is_none()
    }

    pub(crate) fn result_ready(&self) -> bool {
        self.result
            .as_ref()
            .map(|r| r.err.is_none() && r.table.col_count() > 0)
            .unwrap_or(false)
    }

    pub(crate) fn can_enter_step(&self, step: WorkflowStep) -> bool {
        match step {
            WorkflowStep::Sources => true,
            WorkflowStep::Configure => self.sources_ready(),
            // 连接进行中也允许回到结果页看进度占位。
            WorkflowStep::Result => self.join_active || self.result_ready(),
        }
    }

    pub(crate) fn go_to_step(&mut self, step: WorkflowStep) {
        if self.can_enter_step(step) {
            self.step = step;
        }
    }
}
