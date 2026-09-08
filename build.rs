//! 构建脚本:仅 Windows 目标时,把 assets/icon.ico 嵌入 .exe 资源
//! (资源管理器 / 任务栏 / Alt-Tab 图标)。Linux / CI arm64 构建直接跳过。

use std::path::PathBuf;

fn main() {
    // 只在 Windows 目标嵌入图标(交叉编译 target=x86_64-pc-windows-gnu 也适用)
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let ico = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/icon.ico");
    if !ico.exists() {
        panic!("缺少图标资源: {}", ico.display());
    }

    // 生成 windres 资源脚本
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let rc = out_dir.join("app.rc");
    std::fs::write(
        &rc,
        format!(
            "1 ICON \"{}\"\n",
            ico.to_str().expect("图标路径需为 UTF-8")
        ),
    )
    .expect("写入 app.rc 失败");

    // 定位交叉编译器自带 / 系统的 windres
    let windres = std::env::var("WINDRES")
        .ok()
        .filter(|p| !p.is_empty())
        .or_else(|| {
            let cc = std::env::var("CC").unwrap_or_default();
            cc.strip_suffix("gcc")
                .map(|prefix| format!("{}windres", prefix))
        })
        .unwrap_or_else(|| "x86_64-w64-mingw32-windres".to_string());

    let obj = out_dir.join("app_res.o");
    let status = std::process::Command::new(&windres)
        .arg(&rc)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&obj)
        .status()
        .unwrap_or_else(|e| panic!("调用 windres 失败({}): {}", windres, e));
    assert!(status.success(), "windres 编译资源失败");

    // 链接进最终 exe
    println!(
        "cargo:rustc-link-arg-bin=excelookup={}",
        obj.display()
    );
    // 资源变更时重跑
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
}
