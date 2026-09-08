//! 表格数据模型:跨 GUI/引擎的核心数据结构

/// 单元格值:统一为字符串或数值,便于 join 与展示
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    /// 数值(整数或浮点,统一存 f64;单元格显示格式在输出层决定)
    Number(f64),
    /// 文本
    Text(String),
    /// 空
    Empty,
}

impl CellValue {
    /// 展示用文本(空 → "")
    pub fn display(&self) -> String {
        match self {
            CellValue::Number(n) => {
                if n.fract() == 0.0 && n.is_finite() {
                    // 整数显示不带小数点
                    format!("{:.0}", n)
                } else {
                    n.to_string()
                }
            }
            CellValue::Text(s) => s.clone(),
            CellValue::Empty => String::new(),
        }
    }
}

impl From<&str> for CellValue {
    fn from(s: &str) -> Self {
        CellValue::Text(s.to_owned())
    }
}

impl From<String> for CellValue {
    fn from(s: String) -> Self {
        CellValue::Text(s)
    }
}

impl From<f64> for CellValue {
    fn from(n: f64) -> Self {
        CellValue::Number(n)
    }
}

/// 一张已读入的表:列名 + 行数据
#[derive(Debug, Clone, Default)]
pub struct Table {
    /// 列名(与每行 cells 对齐)
    pub headers: Vec<String>,
    /// 每行 = 一列一个 CellValue
    pub rows: Vec<Vec<CellValue>>,
}

impl Table {
    pub fn new(headers: Vec<String>) -> Self {
        Self {
            headers,
            rows: Vec::new(),
        }
    }

    pub fn col_count(&self) -> usize {
        self.headers.len()
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// 取某行某列(越界返回 None)
    pub fn cell(&self, row: usize, col: usize) -> Option<&CellValue> {
        self.rows.get(row).and_then(|r| r.get(col))
    }

    /// 表是否为空(无行或无列)
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() || self.headers.is_empty()
    }

    /// 追加一行(长度自动对齐 headers;不足补 Empty,超出截断)
    pub fn push_row(&mut self, mut row: Vec<CellValue>) {
        let w = self.col_count();
        if row.len() < w {
            row.resize(w, CellValue::Empty);
        } else if row.len() > w {
            row.truncate(w);
        }
        self.rows.push(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_row_aligns() {
        let mut t = Table::new(vec!["a".into(), "b".into(), "c".into()]);
        t.push_row(vec!["x".into(), "y".into()]); // 少一列 → 补 Empty
        t.push_row(vec!["1".into(), "2".into(), "3".into(), "4".into()]); // 多一列 → 截断
        assert_eq!(t.row_count(), 2);
        assert_eq!(t.cell(0, 2), Some(&CellValue::Empty));
        assert_eq!(t.cell(1, 2), Some(&CellValue::Text("3".into())));
    }

    #[test]
    fn display_number() {
        assert_eq!(CellValue::Number(42.0).display(), "42");
        assert_eq!(CellValue::Number(3.14).display(), "3.14");
        assert_eq!(CellValue::Text("hi".into()).display(), "hi");
        assert_eq!(CellValue::Empty.display(), "");
    }
}
