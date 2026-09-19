//! 集成测试共用的临时目录。
//!
//! 目录名带进程号,避免并行跑测试时多个用例共用一个固定文件名互相踩 ——
//! `Workbook::save` 是 `File::create`(先把文件截断成 0 字节)再写内容,另一边
//! 正好读就会拿到空文件。清理交给 `Drop`:用例正常结束和 panic 展开都会删目录,
//! 不必在每个用例末尾手写 `remove_dir_all`(漏写一次就会在 /tmp 里留一堆残留)。

use std::ops::Deref;
use std::path::{Path, PathBuf};

/// 独占的临时目录,离开作用域时递归删除。
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// 建目录前先清掉同名残留(上一次跑崩在同一 tag 上时留下的)。
    pub fn new(prefix: &str, tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
