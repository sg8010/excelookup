//! ExcelLookup 核心库:数据模型、Excel 读写、join 引擎
//!
//! 独立于 GUI,便于单元测试与未来扩展(CLI 等)。

pub mod export;
pub mod join;
pub mod model;
pub mod read_xlsx;
