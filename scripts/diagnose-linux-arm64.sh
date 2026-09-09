#!/usr/bin/env bash
# ExcelLookup Linux arm64 启动崩溃诊断
# 用法: ./scripts/diagnose-linux-arm64.sh /path/to/excelookup-linux-arm64
set -u

BIN="${1:-}"
if [[ -z "$BIN" ]]; then
    if [[ -f ./excelookup-linux-arm64 ]]; then BIN=./excelookup-linux-arm64
    elif [[ -f ./excelookup ]]; then BIN=./excelookup
    elif [[ -f ./target/release/excelookup ]]; then BIN=./target/release/excelookup
    elif [[ -f ./target/debug/excelookup ]]; then BIN=./target/debug/excelookup
    fi
fi

if [[ -z "$BIN" || ! -f "$BIN" ]]; then
    echo "用法: $0 /path/to/excelookup-linux-arm64 [报告文件]"
    echo "未找到可执行文件。"
    exit 2
fi

if [[ "$BIN" != /* ]]; then
    BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
fi

OUT="${2:-./excelookup-diagnose-$(date +%Y%m%d-%H%M%S).txt}"
TMP_DIR="$(mktemp -d 2>/dev/null || printf '/tmp/excelookup-diagnose-%s' "$$")"
mkdir -p "$TMP_DIR"
trap 'rm -rf "$TMP_DIR"' EXIT
exec > >(tee "$OUT") 2>&1

section() { printf '\n===== %s =====\n' "$1"; }
has_cmd() { command -v "$1" >/dev/null 2>&1; }

section "诊断信息"
date
echo "程序: $BIN"
echo "报告: $OUT"
id 2>/dev/null || true
uname -a 2>/dev/null || true
if [[ -f /etc/os-release ]]; then sed -n '1,12p' /etc/os-release; fi
env | grep -E '^(DISPLAY|WAYLAND_DISPLAY|XDG_SESSION_TYPE|XDG_CURRENT_DESKTOP|DBUS_SESSION_BUS_ADDRESS|XAUTHORITY|GDK_BACKEND)=' || true

section "工具"
has_cmd file && echo "file: $(command -v file)" || echo "file: 未找到"
has_cmd readelf && echo "readelf: $(command -v readelf)" || echo "readelf: 未找到"
has_cmd objdump && echo "objdump: $(command -v objdump)" || echo "objdump: 未找到"
has_cmd ldd && echo "ldd: $(command -v ldd)" || echo "ldd: 未找到"
has_cmd gdb && echo "gdb: $(command -v gdb)" || echo "gdb: 未找到"
has_cmd timeout && echo "timeout: $(command -v timeout)" || echo "timeout: 未找到"
has_cmd gdbus && echo "gdbus: $(command -v gdbus)" || echo "gdbus: 未找到"
has_cmd zenity && echo "zenity: $(command -v zenity)" || echo "zenity: 未找到"

section "程序文件与动态库"
if has_cmd file; then file "$BIN" || true; fi
if has_cmd readelf; then
    echo "--- NEEDED ---"
    readelf -d "$BIN" 2>&1 | grep NEEDED || true
fi
if has_cmd ldd; then
    echo "--- GTK/GDK/GLib/X11 依赖及缺失项 ---"
    ldd "$BIN" 2>&1 | grep -E 'gtk|gdk|glib|gobject|gio|pango|cairo|atk|X11|xcb|not found' || true
fi

check_lib() {
    local name="$1"
    local path="$(ldconfig -p 2>/dev/null | awk -v n="$name" '$1 == n { print $NF; exit }')"
    if [[ -n "$path" ]]; then echo "$name -> $path"; else echo "$name -> 未找到"; fi
}

section "GTK3 运行库"
if has_cmd ldconfig; then
    check_lib libgtk-3.so.0
    check_lib libgdk-3.so.0
    check_lib libglib-2.0.so.0
    check_lib libgobject-2.0.so.0
    check_lib libgio-2.0.so.0
else
    echo "未找到 ldconfig。"
fi
if has_cmd pkg-config; then
    echo "pkg-config GTK3:"
    pkg-config --modversion gtk+-3.0 2>&1 || true
else
    echo "pkg-config: 未找到"
fi

section "已安装包版本"
if has_cmd dpkg-query; then
    dpkg-query -W -f='${binary:Package} ${Version}\n' \
        'libgtk-3-0*' 'libgtk-3-dev' 'libgdk-3-0*' 2>/dev/null || true
fi
if has_cmd rpm; then
    rpm -q --qf='%{NAME} %{VERSION}-%{RELEASE}\n' \
        gtk3 gtk3-devel 2>/dev/null || true
fi

section "D-Bus Portal"
if has_cmd gdbus; then
    portal_log="$TMP_DIR/portal.log"
    if timeout 8s gdbus introspect \
        --session --dest org.freedesktop.portal.Desktop \
        --object-path /org/freedesktop/portal/desktop \
        >"$portal_log" 2>&1; then
        echo "Portal 服务可访问。"
        sed -n '1,12p' "$portal_log"
    else
        echo "Portal 服务不可访问或未注册。"
        sed -n '1,8p' "$portal_log"
    fi
else
    echo "未找到 gdbus,跳过 Portal 检查。"
fi

run_gdb() {
    local label="$1"
    local log_file="$2"
    local backend="${3:-}"
    echo "--- $label ---"
    if ! has_cmd gdb; then echo "未找到 gdb,跳过。"; return; fi

    if has_cmd timeout; then
        if [[ -n "$backend" ]]; then
            timeout --foreground --kill-after=5s 20s env \
                LD_BIND_NOW=1 GDK_BACKEND="$backend" \
                gdb -q -nx -batch \
                -ex 'set pagination off' -ex 'set confirm off' \
                -ex 'set print thread-events off' -ex run \
                -ex 'info program' -ex 'bt full' \
                -ex 'thread apply all bt full' -ex 'info sharedlibrary' \
                -ex quit --args "$BIN" >"$log_file" 2>&1
        else
            timeout --foreground --kill-after=5s 20s env \
                LD_BIND_NOW=1 \
                gdb -q -nx -batch \
                -ex 'set pagination off' -ex 'set confirm off' \
                -ex 'set print thread-events off' -ex run \
                -ex 'info program' -ex 'bt full' \
                -ex 'thread apply all bt full' -ex 'info sharedlibrary' \
                -ex quit --args "$BIN" >"$log_file" 2>&1
        fi
        rc=$?
    else
        echo "未找到 timeout,跳过自动启动测试。"
        return
    fi

    echo "gdb/timeout 返回码: $rc"
    if grep -qE 'SIGSEGV|SIGABRT|SIGBUS|Program received signal|Segmentation fault' "$log_file"; then
        echo "检测到崩溃信号。"
    elif [[ "$rc" == 124 ]]; then
        echo "20 秒内未退出,可能程序已正常显示窗口。"
    else
        echo "未检测到 SIGSEGV/SIGABRT/SIGBUS。"
    fi
    sed -n '1,260p' "$log_file"
}

section "GTK3 实际启动测试"
echo "每次测试最多 20 秒;如果窗口出现,请手动关闭。"
run_gdb "当前环境" "$TMP_DIR/gdb-default.log"
run_gdb "强制 GDK_BACKEND=x11" "$TMP_DIR/gdb-x11.log" x11

section "诊断结束"
echo "完整报告已保存: $OUT"
echo "请将该报告文件内容发回。"
