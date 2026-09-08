//! 端到端集成测试:真实 .xlsx 文件走 造数 → 读 → join → 导出 → 读回验证

use std::path::PathBuf;

use rust_xlsxwriter::Workbook;

use excelookup_lib::export::write_xlsx;
use excelookup_lib::join::{join, JoinSpec, JoinType, KeyMode};
use excelookup_lib::model::CellValue;
use excelookup_lib::read_xlsx::read_workbook;

/// 生成一个临时 xlsx:两个 sheet,left 与 right
fn make_wb(path: &PathBuf) {
    let mut wb = Workbook::new();

    // left sheet
    {
        let s = wb.add_worksheet();
        s.set_name("left").unwrap();
        s.write_string(0, 0, "id").unwrap();
        s.write_string(0, 1, "姓名").unwrap();
        s.write_string(1, 0, "1").unwrap();
        s.write_string(1, 1, "张三").unwrap();
        s.write_string(2, 0, "2").unwrap();
        s.write_string(2, 1, "李四").unwrap();
        s.write_string(3, 0, "3").unwrap();
        s.write_string(3, 1, "王五").unwrap();
    }
    // right sheet: id(数值!) + 部门
    {
        let s = wb.add_worksheet();
        s.set_name("right").unwrap();
        s.write_string(0, 0, "id").unwrap();
        s.write_string(0, 1, "部门").unwrap();
        s.write_number(1, 0, 1.0).unwrap(); // 数值 1,与 left 的文本 "1" 需 Normalize
        s.write_string(1, 1, "工程部").unwrap();
        s.write_number(2, 0, 3.0).unwrap();
        s.write_string(2, 1, "产品部").unwrap();
        s.write_number(3, 0, 9.0).unwrap();
        s.write_string(3, 1, "财务部").unwrap();
    }
    wb.save(path).unwrap();
}

#[test]
fn end_to_end_left_join_real_xlsx() {
    let dir = std::env::temp_dir();
    let src = dir.join("exlook_it_left.xlsx");
    let out = dir.join("exlook_it_result.xlsx");
    make_wb(&src);

    // 1. 读两个 sheet
    let sheets = read_workbook(&src).unwrap();
    assert_eq!(sheets.len(), 2);
    let (lname, left) = sheets.iter().find(|(n, _)| n == "left").unwrap().clone();
    let (_rname, right) = sheets.iter().find(|(n, _)| n == "right").unwrap().clone();
    let (lname, left) = (lname, left);
    let _ = lname;
    assert_eq!(left.row_count(), 3);
    assert_eq!(left.headers, vec!["id", "姓名"]);
    // 数值 id 读成 Number
    assert_eq!(right.row_count(), 3);

    // 2. join(Normalize 让文本 "1" 匹配数值 1)
    let spec = JoinSpec {
        join_type: JoinType::Left,
        left_keys: vec![0],
        right_keys: vec![0],
        right_pick: vec![1],
        key_mode: KeyMode::NORMALIZE,
    };
    let res = join(&left, &right, &spec);
    assert_eq!(res.table.headers, vec!["id", "姓名", "部门"]);
    assert_eq!(res.table.row_count(), 3); // 左表全保留
    assert_eq!(res.left_matched, 2); // id 1,3 命中;2 未命中
    assert_eq!(res.table.cell(0, 2), Some(&CellValue::Text("工程部".into())));
    assert_eq!(res.table.cell(1, 2), Some(&CellValue::Empty)); // 李四未匹配
    assert_eq!(res.table.cell(2, 2), Some(&CellValue::Text("产品部".into())));

    // 3. 导出
    write_xlsx(&res.table, &out).unwrap();

    // 4. 读回验证
    let back = read_workbook(&out).unwrap();
    assert_eq!(back.len(), 1);
    let (_, t2) = &back[0];
    assert_eq!(t2.headers, vec!["id", "姓名", "部门"]);
    assert_eq!(t2.row_count(), 3);
    assert_eq!(t2.cell(0, 2), Some(&CellValue::Text("工程部".into())));
    assert_eq!(t2.cell(1, 2), Some(&CellValue::Empty));

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn end_to_end_inner_join_real_xlsx() {
    let dir = std::env::temp_dir();
    let src = dir.join("exlook_it_inner.xlsx");
    let out = dir.join("exlook_it_inner_result.xlsx");
    make_wb(&src);

    let sheets = read_workbook(&src).unwrap();
    let left = sheets.iter().find(|(n, _)| n == "left").unwrap().1.clone();
    let right = sheets.iter().find(|(n, _)| n == "right").unwrap().1.clone();

    let spec = JoinSpec {
        join_type: JoinType::Inner,
        left_keys: vec![0],
        right_keys: vec![0],
        right_pick: vec![1],
        key_mode: KeyMode::NORMALIZE,
    };
    let res = join(&left, &right, &spec);
    assert_eq!(res.table.row_count(), 2); // id 1,3 命中;2、9 被丢弃

    write_xlsx(&res.table, &out).unwrap();
    let back = read_workbook(&out).unwrap();
    assert_eq!(back[0].1.row_count(), 2);

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
}

/// 同一 Excel 文件的两个 sheet 互 join(订单表 + 客户表)
#[test]
fn end_to_end_same_file_two_sheets() {
    let dir = std::env::temp_dir();
    let path = dir.join("exlook_it_samefile.xlsx");

    // 造文件:sheet1 订单,sheet2 客户
    {
        let mut wb = Workbook::new();
        {
            let s = wb.add_worksheet();
            s.set_name("订单").unwrap();
            for (i, h) in ["订单号", "客户", "金额"].iter().enumerate() {
                s.write_string(0, i as u16, *h).unwrap();
            }
            for (r, row) in [("A001", "张三", "100"), ("A002", "李四", "250"), ("A003", "王五", "80")]
                .iter()
                .enumerate()
            {
                s.write_string((r + 1) as u32, 0, row.0).unwrap();
                s.write_string((r + 1) as u32, 1, row.1).unwrap();
                s.write_string((r + 1) as u32, 2, row.2).unwrap();
            }
        }
        {
            let s = wb.add_worksheet();
            s.set_name("客户").unwrap();
            for (i, h) in ["客户", "城市"].iter().enumerate() {
                s.write_string(0, i as u16, *h).unwrap();
            }
            for (r, row) in [("张三", "北京"), ("李四", "上海"), ("赵六", "广州")].iter().enumerate() {
                s.write_string((r + 1) as u32, 0, row.0).unwrap();
                s.write_string((r + 1) as u32, 1, row.1).unwrap();
            }
        }
        wb.save(&path).unwrap();
    }

    // 读回:两个 sheet 都应存在
    let sheets = read_workbook(&path).unwrap();
    assert_eq!(sheets.len(), 2);
    let find = |n: &str| sheets.iter().find(|(x, _)| x == n).unwrap().1.clone();
    let orders = find("订单");
    let customers = find("客户");
    assert_eq!(orders.headers, vec!["订单号", "客户", "金额"]);
    assert_eq!(orders.row_count(), 3);
    assert_eq!(customers.headers, vec!["客户", "城市"]);

    // join:订单.客户 = 客户.客户,取城市
    let spec = JoinSpec {
        join_type: JoinType::Left,
        left_keys: vec![1], // 客户列
        right_keys: vec![0],
        right_pick: vec![1], // 城市
        key_mode: KeyMode::EXACT,
    };
    let res = join(&orders, &customers, &spec);
    assert_eq!(res.table.headers, vec!["订单号", "客户", "金额", "城市"]);
    assert_eq!(res.table.row_count(), 3); // 左表全保留
    assert_eq!(
        res.table.cell(0, 3),
        Some(&CellValue::Text("北京".into()))
    );
    assert_eq!(
        res.table.cell(1, 3),
        Some(&CellValue::Text("上海".into()))
    );
    // 王五未在客户表 → 城市空
    assert_eq!(res.table.cell(2, 3), Some(&CellValue::Empty));
    assert_eq!(res.left_matched, 2);

    let _ = std::fs::remove_file(&path);
}
