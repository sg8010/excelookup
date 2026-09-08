#!/usr/bin/env bash
# 交叉编译 Windows x64 版 ExcelLookup
# 依赖: mingw-w64 (sudo apt install -y gcc-mingw-w64-x86-64)
set -euo pipefail
cd "$(dirname "$0")/.."

export PATH="$HOME/.cargo/bin:$PATH"
# 代理(如需要)
export HTTP_PROXY=http://127.0.0.1:10808 HTTPS_PROXY=http://127.0.0.1:10808

rustup target add x86_64-pc-windows-gnu 2>/dev/null || true

cargo build --release --target x86_64-pc-windows-gnu

EXE=target/x86_64-pc-windows-gnu/release/excelookup.exe
echo ""
echo "✅ 构建完成: $EXE ($(du -h "$EXE" | cut -f1))"
echo "   运行验证: 复制到 Windows 后双击, 或 (cd /mnt/c && ./excelookup.exe)"
