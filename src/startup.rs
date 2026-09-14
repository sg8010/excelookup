//! 启动阶段诊断。
//!
//! eframe 在创建窗口和 OpenGL 上下文前不会显示应用自己的界面。桌面菜单启动时，
//! `Terminal=false` 又会把标准错误隐藏起来，因此这里把启动日志写入用户目录，
//! 并在 eframe 返回错误或启动超时时尝试弹出系统错误对话框。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};

/// 图形环境初始化的最长等待时间。
///
/// 正常情况下 eframe 从启动到第一帧远小于这个时间。超时主要用于捕获
/// OpenGL/EGL 驱动调用卡死这一类不会返回 `Result` 的故障。
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// 启动诊断状态。
pub struct StartupDiagnostics {
    log_path: Option<PathBuf>,
    file: Mutex<Option<File>>,
    failure_reported: AtomicBool,
}

impl StartupDiagnostics {
    /// 创建本次启动的诊断日志并记录运行环境。
    pub fn begin() -> Arc<Self> {
        let (log_path, file) = open_log_file();
        let diagnostics = Arc::new(Self {
            log_path,
            file: Mutex::new(file),
            failure_reported: AtomicBool::new(false),
        });

        // eframe/glutin 使用 log crate 输出的 OpenGL 初始化细节对定位问题很有用。
        // 若其他组件已经注册 logger,不影响程序启动,仍会保留本模块的直接日志。
        if log::set_boxed_logger(Box::new(FileLogger(Arc::clone(&diagnostics)))).is_ok() {
            log::set_max_level(log::LevelFilter::Debug);
        }

        diagnostics.write_line("启动诊断开始");
        diagnostics.write_environment();
        diagnostics
    }

    /// 启动日志路径,用于错误提示和用户反馈。
    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }

    /// 写入一条不依赖 log logger 的诊断信息。
    pub fn write_line(&self, message: &str) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let line = format!("[{timestamp}] {message}\n");

        let Ok(mut guard) = self.file.lock() else {
            return;
        };
        let Some(file) = guard.as_mut() else {
            return;
        };
        // 诊断日志的首要目标是记录卡死前最后一步,每行立即 flush。
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }

    /// 启动一个轻量 watchdog。第一帧 UI 到达后会自动结束等待。
    pub fn spawn_watchdog(self: &Arc<Self>, ready: Arc<AtomicBool>) {
        let diagnostics = Arc::clone(self);
        std::thread::spawn(move || {
            // 分段等待,这样第一帧到达后无需等满 30 秒才结束线程。
            let slices = STARTUP_TIMEOUT.as_millis() / 250;
            for _ in 0..slices {
                if ready.load(Ordering::Acquire)
                    || diagnostics.failure_reported.load(Ordering::Acquire)
                {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }

            if ready.load(Ordering::Acquire) {
                return;
            }

            let first_report = diagnostics.report_failure(
                "启动超过 30 秒仍未显示首个界面。程序可能卡在 X11 或 OpenGL/EGL 图形环境初始化阶段。",
            );
            if first_report {
                // 已经弹出错误提示后结束卡住的进程,避免桌面启动器一直显示忙碌状态。
                std::process::exit(1);
            }
        });
    }

    /// 安装 panic hook,确保 app creator 或首帧前的 panic 也能留下原因。
    pub fn install_panic_hook(self: &Arc<Self>, ready: &Arc<AtomicBool>) {
        let previous = std::panic::take_hook();
        let diagnostics = Arc::clone(self);
        let ready = Arc::clone(ready);
        std::panic::set_hook(Box::new(move |info| {
            let detail = panic_detail(info);
            if ready.load(Ordering::Acquire) {
                diagnostics.write_line(&format!("程序运行期间发生异常: {detail}"));
                eprintln!("程序运行期间发生异常: {detail}");
            } else {
                diagnostics.report_failure(&format!("启动期间发生异常: {detail}"));
            }
            previous(info);
        }));
    }

    /// 记录失败并尽量直接显示错误。返回 `true` 表示本次调用首次报告。
    pub fn report_failure(&self, error: &str) -> bool {
        if self.failure_reported.swap(true, Ordering::AcqRel) {
            // watchdog 先报超时、eframe 随后返回具体错误时,具体错误仍需进入日志。
            self.write_line(&format!("后续启动错误: {error}"));
            return false;
        }

        let message = failure_message(error, self.log_path());
        self.write_line(&format!("启动失败: {message}"));
        eprintln!("{message}");
        show_failure_dialog(&message, self.log_path());
        true
    }

    fn write_environment(&self) {
        let version = option_env!("EXCELOOKUP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
        self.write_line(&format!("版本: {version}"));
        self.write_line(&format!(
            "目标: {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
        self.write_line(&format!(
            "可执行文件: {}",
            std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "<无法获取>".to_owned())
        ));
        self.write_line(&format!(
            "工作目录: {}",
            std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "<无法获取>".to_owned())
        ));

        for key in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_SESSION_TYPE",
            "XDG_CURRENT_DESKTOP",
            "XDG_RUNTIME_DIR",
            "LD_LIBRARY_PATH",
        ] {
            let value = std::env::var_os(key)
                .map(|value| value.to_string_lossy().into_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "<未设置>".to_owned());
            self.write_line(&format!("环境变量 {key}: {value}"));
        }

        #[cfg(target_os = "linux")]
        {
            if let Ok(kernel) = fs::read_to_string("/proc/sys/kernel/osrelease") {
                self.write_line(&format!("Linux 内核: {}", kernel.trim()));
            }
            if let Ok(os_release) = fs::read_to_string("/etc/os-release") {
                let summary = os_release
                    .lines()
                    .filter(|line| line.starts_with("PRETTY_NAME=") || line.starts_with("NAME="))
                    .take(2)
                    .collect::<Vec<_>>()
                    .join("; ");
                if !summary.is_empty() {
                    self.write_line(&format!("发行版: {summary}"));
                }
            }
            self.write_linux_library_status();
        }
    }

    #[cfg(target_os = "linux")]
    fn write_linux_library_status(&self) {
        // glutin、winit 为了兼容不同发行版通过 dlopen 加载这些库,所以 ldd
        // 看不到它们;这里仅作诊断记录,不把探测失败当作程序启动硬错误。
        const LIBRARIES: &[&str] = &[
            "libGL.so.1",
            "libEGL.so.1",
            "libX11.so.6",
            "libX11-xcb.so.1",
            "libxcb.so.1",
            "libxkbcommon.so.0",
            "libxkbcommon-x11.so.0",
            "libXcursor.so.1",
            "libXi.so.6",
            "libXrandr.so.2",
            "libXfixes.so.3",
            "libXrender.so.1",
        ];

        let output = ["/sbin/ldconfig", "ldconfig"].iter().find_map(|program| {
            Command::new(program)
                .arg("-p")
                .output()
                .ok()
                .filter(|output| output.status.success())
        });
        let Some(output) = output else {
            self.write_line("动态库检查: 无法执行 ldconfig -p");
            return;
        };
        let listing = String::from_utf8_lossy(&output.stdout);
        self.write_line("动态库检查(来自 ldconfig -p):");
        for library in LIBRARIES {
            let found = listing.lines().any(|line| line.contains(library));
            self.write_line(&format!(
                "  {library}: {}",
                if found { "存在" } else { "未找到" }
            ));
        }
    }
}

struct FileLogger(Arc<StartupDiagnostics>);

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Debug
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.0
                .write_line(&format!("[{}] {}", record.level(), record.args()));
        }
    }

    fn flush(&self) {}
}

fn open_log_file() -> (Option<PathBuf>, Option<File>) {
    let mut candidates = Vec::new();
    if let Some(state_home) = non_empty_env_path("XDG_STATE_HOME") {
        candidates.push(state_home.join("excelookup/startup.log"));
    }
    if let Some(home) = non_empty_env_path("HOME") {
        candidates.push(home.join(".cache/excelookup/startup.log"));
    }
    candidates.push(PathBuf::from("/tmp/excelookup-startup.log"));

    for path in candidates {
        if let Some(parent) = path.parent()
            && fs::create_dir_all(parent).is_err()
        {
            continue;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path);
        if let Ok(file) = file {
            return (Some(path), Some(file));
        }
    }
    (None, None)
}

fn non_empty_env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn panic_detail(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("未知 panic 信息");
    match info.location() {
        Some(location) => format!("{payload} ({location})"),
        None => payload.to_owned(),
    }
}

fn failure_message(error: &str, log_path: Option<&Path>) -> String {
    let hint = explain_error(error);
    let log = log_path
        .map(|path| format!("诊断日志: {}", path.display()))
        .unwrap_or_else(|| "诊断日志: 无法创建日志文件,请从终端启动并查看错误输出".to_owned());
    format!("ExcelLookup 无法启动。\n\n可能原因:\n{hint}\n\n底层错误:\n{error}\n\n{log}")
}

fn explain_error(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    if error.contains("neither wayland")
        || error.contains("display is not set")
        || error.contains("wayland_display")
    {
        "未检测到可用的桌面显示会话。请从 UOS 桌面启动,并确认 DISPLAY 已设置;当前版本使用 X11,不支持仅有 Wayland 会话。"
    } else if error.contains("xkbcommon-x11") {
        "无法加载 libxkbcommon-x11.so,这是 UOS X11 输入支持的运行库。请安装发行版对应的 libxkbcommon-x11-0 运行包后重试。"
    } else if error.contains("libgl.so") || error.contains("libegl.so") {
        "无法加载 OpenGL/EGL 运行库。请检查 UOS 的 libgl1、libegl1 及显卡/Mesa 驱动是否完整安装。"
    } else if error.contains("xcursor")
        || error.contains("xkbcommon")
        || error.contains("x11")
        || error.contains("xcb")
    {
        "X11 运行库或 X11 扩展可能缺失/不可用,请检查 libX11、libxcb、libxkbcommon、libXcursor 等运行库。"
    } else if error.contains("glutin")
        || error.contains("opengl")
        || error.contains("egl")
        || error.contains("glx")
        || error.contains("shader")
        || error.contains("gl context")
    {
        "OpenGL/EGL/GLX 图形环境初始化失败,常见原因是显卡驱动、Mesa/图形运行库缺失或当前硬件不支持所需 OpenGL。"
    } else {
        "窗口或图形渲染环境初始化失败。请检查 UOS 桌面会话、X11/图形运行库及显卡驱动;诊断日志包含更具体的底层信息。"
    }
}

#[cfg(target_os = "linux")]
fn show_failure_dialog(message: &str, log_path: Option<&Path>) {
    // 没有图形会话时,zenity 等工具可能等待很久才退出;此时直接保留 stderr
    // 和日志即可,不要让错误处理本身看起来像又卡住了。
    let has_display = ["DISPLAY", "WAYLAND_DISPLAY"].iter().any(|key| {
        std::env::var_os(key)
            .map(|value| !value.is_empty())
            .unwrap_or(false)
    });
    if !has_display {
        return;
    }

    let title = "ExcelLookup 启动失败";
    // 不把 zenity 等工具作为正常运行依赖;只在图形初始化失败、程序无法创建自己
    // 的窗口时尝试使用系统已有的错误提示工具。
    let dialogs: &[(&str, &[&str])] = &[
        (
            "zenity",
            &[
                "--error",
                "--no-markup",
                "--title",
                title,
                "--width",
                "760",
                "--text",
                message,
            ],
        ),
        ("kdialog", &["--title", title, "--error", message]),
        ("xmessage", &["-center", "-title", title, message]),
    ];
    for (program, args) in dialogs {
        if run_dialog(program, args) {
            return;
        }
    }

    // 没有模态对话框工具时,通知和打开日志仍比静默退出更容易让用户发现原因。
    if run_dialog("notify-send", &["--urgency=critical", title, message]) {
        return;
    }

    // 模态对话框都不可用时,直接打开本次启动实际使用的日志文件。
    let mut log_paths = Vec::new();
    if let Some(path) = log_path {
        log_paths.push(path.to_path_buf());
    }
    log_paths.extend(
        [
            Some(PathBuf::from("/tmp/excelookup-startup.log")),
            non_empty_env_path("XDG_STATE_HOME").map(|p| p.join("excelookup/startup.log")),
            non_empty_env_path("HOME").map(|p| p.join(".cache/excelookup/startup.log")),
        ]
        .into_iter()
        .flatten(),
    );
    for path in log_paths {
        if path.is_file() && spawn_detached("xdg-open", &[path.as_os_str()]) {
            return;
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn show_failure_dialog(_message: &str, _log_path: Option<&Path>) {}

#[cfg(target_os = "linux")]
fn run_dialog(program: &str, args: &[&str]) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };

    // 图形对话框打开后不能等待用户关闭,否则主进程不会及时结束;只等待很短时间
    // 判断工具是否因缺少库/无效显示变量立即失败。仍在运行则视为已成功交给桌面。
    for _ in 0..6 {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return false,
        }
    }
    true
}

#[cfg(target_os = "linux")]
fn spawn_detached(program: &str, args: &[&std::ffi::OsStr]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_missing_display() {
        assert!(explain_error("neither WAYLAND_DISPLAY nor DISPLAY is set").contains("显示会话"));
    }

    #[test]
    fn explains_graphics_failure() {
        assert!(explain_error("glutin error: failed to create EGL context").contains("OpenGL"));
    }

    #[test]
    fn explains_missing_xkbcommon_x11() {
        assert!(
            explain_error("Library libxkbcommon-x11.so could not be loaded")
                .contains("libxkbcommon-x11-0")
        );
    }

    #[test]
    fn failure_message_contains_raw_error_and_log() {
        let message = failure_message("测试底层错误", Some(Path::new("/tmp/test.log")));
        assert!(message.contains("测试底层错误"));
        assert!(message.contains("/tmp/test.log"));
    }
}
