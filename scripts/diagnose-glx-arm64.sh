#!/usr/bin/env bash
# 目标机 ARM64 GLX/OpenGL/X Server 诊断
# 用法: ./diagnose-glx-arm64.sh [报告文件]
set -u

OUT="${1:-./excelookup-glx-diagnose-$(date +%Y%m%d-%H%M%S).txt}"
TMP_DIR="$(mktemp -d 2>/dev/null || printf '/tmp/excelookup-glx-%s' "$$")"
mkdir -p "$TMP_DIR"
trap 'rm -rf "$TMP_DIR"' EXIT
exec > >(tee "$OUT") 2>&1

section() { printf '\n===== %s =====\n' "$1"; }
has_cmd() { command -v "$1" >/dev/null 2>&1; }

section "基本环境"
date
uname -a 2>/dev/null || true
id 2>/dev/null || true
if [[ -f /etc/os-release ]]; then sed -n '1,12p' /etc/os-release; fi
env | grep -E '^(DISPLAY|WAYLAND_DISPLAY|XDG_SESSION_TYPE|XDG_CURRENT_DESKTOP|DBUS_SESSION_BUS_ADDRESS|XAUTHORITY|LIBGL|MESA|GDK_BACKEND)=' || true

section "工具"
has_cmd ldconfig && echo "ldconfig: $(command -v ldconfig)" || echo "ldconfig: 未找到"
has_cmd ldd && echo "ldd: $(command -v ldd)" || echo "ldd: 未找到"
has_cmd readelf && echo "readelf: $(command -v readelf)" || echo "readelf: 未找到"
has_cmd glxinfo && echo "glxinfo: $(command -v glxinfo)" || echo "glxinfo: 未找到"
has_cmd xdpyinfo && echo "xdpyinfo: $(command -v xdpyinfo)" || echo "xdpyinfo: 未找到"
has_cmd gdb && echo "gdb: $(command -v gdb)" || echo "gdb: 未找到"
has_cmd timeout && echo "timeout: $(command -v timeout)" || echo "timeout: 未找到"

section "GL/GLX/EGL 库搜索路径"
if has_cmd ldconfig; then
    ldconfig -p 2>/dev/null | grep -E 'lib(GLX|GL|EGL)(_|\.so)' || true
else
    echo "无法读取 ldconfig 缓存。"
fi

check_lib() {
    local name="$1"
    local path real owner
    path="$(ldconfig -p 2>/dev/null | awk -v n="$name" '$1 == n { print $NF; exit }')"
    if [[ -z "$path" ]]; then
        echo "$name: 未找到"
        return
    fi
    real="$(readlink -f "$path" 2>/dev/null || printf '%s' "$path")"
    echo "$name:"
    echo "  链接路径: $path"
    echo "  真实文件: $real"
    if has_cmd dpkg-query; then
        owner="$(dpkg-query -S "$real" 2>/dev/null | head -1)"
        [[ -n "$owner" ]] && echo "  Debian 包: $owner"
    fi
    if has_cmd rpm; then
        owner="$(rpm -qf "$real" 2>/dev/null | head -1)"
        [[ -n "$owner" ]] && echo "  RPM 包: $owner"
    fi
    if has_cmd ldd; then
        echo "  关键依赖:"
        ldd "$real" 2>&1 | grep -E 'GLX|GL\.so|EGL|Mesa|X11|xcb|not found' || true
    fi
}

section "关键库归属"
check_lib libGLX.so.0
check_lib libGL.so.1
check_lib libEGL.so.1
check_lib libGLX_mesa.so.0

section "相关已安装包"
if has_cmd dpkg; then
    dpkg -l 2>/dev/null | grep -Ei 'mesa|glvnd|libgl|libegl|libglx|xserver|xorg' | head -120 || true
fi
if has_cmd rpm; then
    rpm -qa 2>/dev/null | grep -Ei 'mesa|glvnd|libgl|egl|glx|xserver|xorg' | head -120 || true
fi

section "X Server 与 GLX 扩展"
ps -eo pid,user,args 2>/dev/null | grep -E '[X]org|[X]wayland|[X]wayland' || true
if has_cmd Xorg; then Xorg -version 2>&1 | head -8; fi
if has_cmd xdpyinfo; then
    echo "--- xdpyinfo GLX ---"
    timeout 10s xdpyinfo -ext GLX 2>&1 | sed -n '1,100p' || true
else
    echo "未找到 xdpyinfo。"
fi

run_glxinfo() {
    local label="$1"
    local log="$2"
    shift 2
    echo "--- $label ---"
    if ! has_cmd glxinfo; then
        echo "未找到 glxinfo,跳过。"
        return
    fi
    if ! has_cmd timeout; then
        echo "未找到 timeout,跳过以防 glxinfo 卡住。"
        return
    fi
    timeout --foreground --kill-after=5s 15s env "$@" glxinfo -B >"$log" 2>&1
    rc=$?
    echo "返回码: $rc"
    sed -n '1,120p' "$log"
    if grep -qE 'SIGSEGV|SIGABRT|SIGBUS|Segmentation fault' "$log"; then
        echo "检测到 glxinfo 崩溃。"
    fi
}

section "OpenGL/GLX 硬件渲染"
run_glxinfo "普通硬件路径" "$TMP_DIR/glxinfo-normal.log" LIBGL_DEBUG=verbose

section "OpenGL/GLX 软件渲染对照"
run_glxinfo "强制软件路径" "$TMP_DIR/glxinfo-software.log" \
    LIBGL_ALWAYS_SOFTWARE=1 LIBGL_DEBUG=verbose

section "glxinfo 崩溃时的 gdb 回溯"
if has_cmd gdb && has_cmd glxinfo && has_cmd timeout; then
    gdb_log="$TMP_DIR/glxinfo-gdb.log"
    timeout --foreground --kill-after=5s 20s \
        env LIBGL_DEBUG=verbose \
        gdb -q -nx -batch \
        -ex 'set pagination off' \
        -ex 'set confirm off' \
        -ex run \
        -ex 'bt full' \
        -ex 'thread apply all bt full' \
        -ex quit --args glxinfo -B >"$gdb_log" 2>&1
    echo "gdb 返回码: $?"
    if grep -qE 'SIGSEGV|SIGABRT|SIGBUS|Program received signal' "$gdb_log"; then
        echo "--- glxinfo gdb 崩溃回溯 ---"
        sed -n '1,220p' "$gdb_log"
    else
        echo "glxinfo 未产生 gdb 崩溃信号。"
    fi
else
    echo "缺少 gdb、glxinfo 或 timeout,跳过。"
fi

section "诊断结束"
echo "报告已保存: $OUT"
