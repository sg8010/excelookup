//! 端到端集成测试:真实 .xlsx 文件走 造数 → 读 → join → 导出 → 读回验证

mod support;

use std::path::PathBuf;

use calamine::{Data, Reader, open_workbook_auto};
use rust_xlsxwriter::Workbook;

use excelookup_lib::export::write_joined_xlsx;
use excelookup_lib::join::{JoinSpec, JoinType, KeyMode, join};
use excelookup_lib::model::CellValue;
use excelookup_lib::read_xlsx::{ReadOptions, read_sheet_opts, read_workbook, read_workbook_opts};

use support::TempDir;

/// 本文件独占的临时目录:名字带用例 tag,离开作用域自动删除。
fn test_dir(tag: &str) -> TempDir {
    TempDir::new("excelookup-it", tag)
}

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
    let dir = test_dir("left");
    let src = dir.join("left.xlsx");
    let out = dir.join("result.xlsx");
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
        left_pick: None,
        right_pick: vec![1],
        key_mode: KeyMode::NORMALIZE,
        expand_dup: true,
    };
    let res = join(&left, &right, &spec);
    assert_eq!(res.table.headers, vec!["id", "姓名", "部门"]);
    assert_eq!(res.table.row_count(), 3); // 左表全保留
    assert_eq!(res.left_matched, 2); // id 1,3 命中;2 未命中
    assert_eq!(
        res.table.cell(&left, &right, 0, 2),
        Some(&CellValue::Text("工程部".into()))
    );
    assert_eq!(res.table.cell(&left, &right, 1, 2), Some(&CellValue::Empty)); // 李四未匹配
    assert_eq!(
        res.table.cell(&left, &right, 2, 2),
        Some(&CellValue::Text("产品部".into()))
    );

    // 3. 导出
    write_joined_xlsx(&res.table, &left, &right, &out).unwrap();

    // 4. 读回验证
    let back = read_workbook(&out).unwrap();
    assert_eq!(back.len(), 1);
    let (_, t2) = &back[0];
    assert_eq!(t2.headers, vec!["id", "姓名", "部门"]);
    assert_eq!(t2.row_count(), 3);
    assert_eq!(t2.cell(0, 2), Some(&CellValue::Text("工程部".into())));
    assert_eq!(t2.cell(1, 2), Some(&CellValue::Empty));
}

#[test]
fn end_to_end_inner_join_real_xlsx() {
    let dir = test_dir("inner");
    let src = dir.join("inner.xlsx");
    let out = dir.join("inner_result.xlsx");
    make_wb(&src);

    let sheets = read_workbook(&src).unwrap();
    let left = sheets.iter().find(|(n, _)| n == "left").unwrap().1.clone();
    let right = sheets.iter().find(|(n, _)| n == "right").unwrap().1.clone();

    let spec = JoinSpec {
        join_type: JoinType::Inner,
        left_keys: vec![0],
        right_keys: vec![0],
        left_pick: None,
        right_pick: vec![1],
        key_mode: KeyMode::NORMALIZE,
        expand_dup: true,
    };
    let res = join(&left, &right, &spec);
    assert_eq!(res.table.row_count(), 2); // id 1,3 命中;2、9 被丢弃

    write_joined_xlsx(&res.table, &left, &right, &out).unwrap();
    let back = read_workbook(&out).unwrap();
    assert_eq!(back[0].1.row_count(), 2);
}

/// 同一 Excel 文件的两个 sheet 互 join(订单表 + 客户表)
#[test]
fn end_to_end_same_file_two_sheets() {
    let dir = test_dir("samefile");
    let path = dir.join("samefile.xlsx");

    // 造文件:sheet1 订单,sheet2 客户
    {
        let mut wb = Workbook::new();
        {
            let s = wb.add_worksheet();
            s.set_name("订单").unwrap();
            for (i, h) in ["订单号", "客户", "金额"].iter().enumerate() {
                s.write_string(0, i as u16, *h).unwrap();
            }
            for (r, row) in [
                ("A001", "张三", "100"),
                ("A002", "李四", "250"),
                ("A003", "王五", "80"),
            ]
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
            for (r, row) in [("张三", "北京"), ("李四", "上海"), ("赵六", "广州")]
                .iter()
                .enumerate()
            {
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
        left_pick: None,
        right_pick: vec![1], // 城市
        key_mode: KeyMode::EXACT,
        expand_dup: true,
    };
    let res = join(&orders, &customers, &spec);
    assert_eq!(res.table.headers, vec!["订单号", "客户", "金额", "城市"]);
    assert_eq!(res.table.row_count(), 3); // 左表全保留
    assert_eq!(
        res.table.cell(&orders, &customers, 0, 3),
        Some(&CellValue::Text("北京".into()))
    );
    assert_eq!(
        res.table.cell(&orders, &customers, 1, 3),
        Some(&CellValue::Text("上海".into()))
    );
    // 王五未在客户表 → 城市空
    assert_eq!(
        res.table.cell(&orders, &customers, 2, 3),
        Some(&CellValue::Empty)
    );
    assert_eq!(res.left_matched, 2);
}

/// 首行是合并单元格大标题、第二行才是列名:指定列名行后应跳过标题行
#[test]
fn end_to_end_header_row_skips_merged_title() {
    // 目录守卫必须绑定:临时值会在语句结束时 Drop,连带把目录删掉。
    let dir = test_dir("title");
    let path = dir.join("title.xlsx");
    {
        let mut wb = Workbook::new();
        let s = wb.add_worksheet();
        s.set_name("销售").unwrap();
        s.merge_range(
            0,
            0,
            0,
            2,
            "2024 年销售统计",
            &rust_xlsxwriter::Format::new(),
        )
        .unwrap();
        for (i, h) in ["id", "名称", "金额"].iter().enumerate() {
            s.write_string(1, i as u16, *h).unwrap();
        }
        for (r, (id, name, amount)) in [
            (1001.0, "苹果", 12.5),
            (1002.0, "香蕉", 7.0),
            (1003.0, "橙子", 3.25),
        ]
        .iter()
        .enumerate()
        {
            let row = (r + 2) as u32;
            s.write_number(row, 0, *id).unwrap();
            s.write_string(row, 1, *name).unwrap();
            s.write_number(row, 2, *amount).unwrap();
        }
        wb.save(&path).unwrap();
    }

    // 指定第 2 行(0-based 1)作列名 → 大标题行不参与连接
    let opts = ReadOptions {
        header_rows: vec![Some(1)],
        preview: true,
    };
    let sheets = read_workbook_opts(&path, opts).unwrap();
    assert_eq!(sheets.len(), 1);
    let sheet = &sheets[0];
    assert_eq!(sheet.name, "销售");
    assert_eq!(sheet.table.headers, vec!["id", "名称", "金额"]);
    assert_eq!(sheet.table.row_count(), 3);
    assert_eq!(sheet.table.cell(0, 0), Some(&CellValue::Number(1001.0)));
    assert_eq!(sheet.auto_header_row, Some(0)); // 自动会被标题行占掉
    assert_eq!(sheet.used_header_row, Some(1));
    assert_eq!(sheet.first_row_number, 1);
    assert_eq!(sheet.preview[0][0], "2024 年销售统计");
    assert_eq!(sheet.preview[1], vec!["id", "名称", "金额"]);

    // 连接:带出"金额"列,行数不受标题行影响
    let spec = JoinSpec {
        left_keys: vec![0],
        right_keys: vec![0],
        left_pick: None,
        right_pick: vec![2],
        join_type: JoinType::Left,
        key_mode: KeyMode::NORMALIZE,
        expand_dup: true,
    };
    let right = sheet.table.clone();
    let res = join(&sheet.table, &right, &spec);
    assert_eq!(res.table.headers, vec!["id", "名称", "金额", "金额"]);
    assert_eq!(res.table.row_count(), 3);
    assert_eq!(res.left_matched, 3);
}

/// 列名行按工作表各记一个:同一个工作簿里两个 sheet 结构不同,互不影响
#[test]
fn end_to_end_header_rows_are_per_sheet() {
    let dir = test_dir("per_sheet");
    let path = dir.join("per_sheet.xlsx");
    {
        let mut wb = Workbook::new();
        // sheet1:首行合并大标题,列名在第 2 行
        {
            let s = wb.add_worksheet();
            s.set_name("带标题").unwrap();
            s.merge_range(0, 0, 0, 1, "汇总", &rust_xlsxwriter::Format::new())
                .unwrap();
            s.write_string(1, 0, "id").unwrap();
            s.write_string(1, 1, "名称").unwrap();
            s.write_string(2, 0, "1").unwrap();
            s.write_string(2, 1, "甲").unwrap();
        }
        // sheet2:首行就是列名
        {
            let s = wb.add_worksheet();
            s.set_name("朴素").unwrap();
            s.write_string(0, 0, "id").unwrap();
            s.write_string(0, 1, "城市").unwrap();
            s.write_string(1, 0, "1").unwrap();
            s.write_string(1, 1, "北京").unwrap();
        }
        wb.save(&path).unwrap();
    }

    // 只给第 1 个 sheet 指定列名行;第 2 个 sheet 自动
    let opts = ReadOptions {
        header_rows: vec![Some(1)],
        preview: true,
    };
    let sheets = read_workbook_opts(&path, opts).unwrap();
    assert_eq!(sheets.len(), 2);

    assert_eq!(sheets[0].name, "带标题");
    assert_eq!(sheets[0].table.headers, vec!["id", "名称"]);
    assert_eq!(sheets[0].table.row_count(), 1);
    assert_eq!(sheets[0].used_header_row, Some(1));

    assert_eq!(sheets[1].name, "朴素");
    assert_eq!(sheets[1].table.headers, vec!["id", "城市"]); // 未被第 1 个 sheet 的选择带偏
    assert_eq!(sheets[1].table.row_count(), 1);
    assert_eq!(sheets[1].used_header_row, Some(0));

    // 改列名行时只读目标工作表,不需要重新构造整个工作簿。
    let single = read_sheet_opts(&path, "带标题", Some(1), true).unwrap();
    assert_eq!(single.name, "带标题");
    assert_eq!(single.table.headers, vec!["id", "名称"]);
    assert_eq!(single.table.row_count(), 1);

    // 反向:给第 2 个 sheet 指定一个它没有的行 → 只有它回退(第 1 个不受影响)
    let opts = ReadOptions {
        header_rows: vec![Some(1), Some(7)],
        preview: true,
    };
    let sheets = read_workbook_opts(&path, opts).unwrap();
    assert_eq!(sheets[0].used_header_row, Some(1));
    assert_eq!(sheets[1].used_header_row, Some(0));
    assert_eq!(sheets[1].table.headers, vec!["id", "城市"]);
}

#[test]
fn selected_left_output_columns_export_without_match_key() {
    let dir = test_dir("selected_left");
    let src = dir.join("selected_left.xlsx");
    make_wb(&src);
    let sheets = read_workbook(&src).unwrap();
    let left = &sheets.iter().find(|(name, _)| name == "left").unwrap().1;
    let right = &sheets.iter().find(|(name, _)| name == "right").unwrap().1;

    // 勾掉匹配列(0),只输出姓名 + 部门。
    let spec = JoinSpec {
        join_type: JoinType::Left,
        left_keys: vec![0],
        right_keys: vec![0],
        left_pick: Some(vec![1]),
        right_pick: vec![1],
        key_mode: KeyMode::NORMALIZE,
        expand_dup: true,
    };
    let result = join(left, right, &spec);
    let out = dir.join("without_key.xlsx");
    write_joined_xlsx(&result.table, left, right, &out).unwrap();
    let back = read_workbook(&out).unwrap();
    let actual = &back[0].1;
    assert_eq!(actual.headers, ["姓名", "部门"]);
    assert_eq!(actual.rows, result.table.materialize(left, right).rows);
    assert_eq!(actual.row_count(), 3);
    assert_eq!(actual.cell(0, 0), Some(&CellValue::from("张三")));
    assert_eq!(actual.cell(0, 1), Some(&CellValue::from("工程部")));
    assert_eq!(actual.cell(1, 1), Some(&CellValue::Empty));

    // 保留匹配列:输出多一列 id,后两列的值与上面一致——证明差异只在匹配列本身。
    let kept = JoinSpec {
        left_pick: Some(vec![0, 1]),
        ..spec
    };
    let kept_result = join(left, right, &kept);
    let kept_out = dir.join("with_key.xlsx");
    write_joined_xlsx(&kept_result.table, left, right, &kept_out).unwrap();
    let kept_back = read_workbook(&kept_out).unwrap();
    let kept_actual = &kept_back[0].1;
    assert_eq!(kept_actual.headers, ["id", "姓名", "部门"]);
    assert_eq!(kept_actual.cell(0, 0), Some(&CellValue::from("1")));
    for row in 0..actual.row_count() {
        assert_eq!(kept_actual.cell(row, 1), actual.cell(row, 0));
        assert_eq!(kept_actual.cell(row, 2), actual.cell(row, 1));
    }
}

/// A 输出列一列都不要:结果只剩 B 列(仍能正常导出并在 Excel 里打开)。
#[test]
fn zero_left_output_columns_export_only_right_columns() {
    let dir = test_dir("zero_left");
    let src = dir.join("zero_left.xlsx");
    let out = dir.join("zero_left_result.xlsx");
    make_wb(&src);
    let sheets = read_workbook(&src).unwrap();
    let left = &sheets.iter().find(|(name, _)| name == "left").unwrap().1;
    let right = &sheets.iter().find(|(name, _)| name == "right").unwrap().1;
    let spec = JoinSpec {
        join_type: JoinType::Left,
        left_keys: vec![0],
        right_keys: vec![0],
        left_pick: Some(vec![]),
        right_pick: vec![1],
        key_mode: KeyMode::NORMALIZE,
        expand_dup: true,
    };
    let result = join(left, right, &spec);
    // 空 left_pick 仍算「有输出列」(B 侧还有),输出列判据本身放行。
    assert!(spec.has_output_columns(left.col_count(), right.col_count()));
    // 但没有任何匹配列在输出中:UI 与 worker 会拦下这份配置;lib 本身不强制,导出仍可验证。
    assert!(!spec.has_key_output(left.col_count(), right.col_count()));
    assert_eq!(result.table.headers, ["部门"]);
    assert_eq!(result.left_matched, 2);
    write_joined_xlsx(&result.table, left, right, &out).unwrap();
    // 按工作表绝对坐标验证,未命中行保持为空且不挤掉后续匹配行。
    assert_eq!(result.table.row_count(), 3);
    let mut workbook = open_workbook_auto(&out).unwrap();
    let actual = workbook.worksheet_range_at(0).unwrap().unwrap();
    assert_eq!(actual.get_size(), (4, 1));
    assert_eq!(actual.get_value((0, 0)), Some(&Data::String("部门".into())));
    assert_eq!(
        actual.get_value((1, 0)),
        Some(&Data::String("工程部".into()))
    );
    assert_eq!(actual.get_value((2, 0)), Some(&Data::Empty));
    assert_eq!(
        actual.get_value((3, 0)),
        Some(&Data::String("产品部".into()))
    );
}

/// 全部越界的 A 输出列等于没选:导出侧不出现空列。
#[test]
fn out_of_range_left_output_columns_export_without_empty_columns() {
    let dir = test_dir("oob_left");
    let src = dir.join("oob_left.xlsx");
    let out = dir.join("oob_left_result.xlsx");
    make_wb(&src);
    let sheets = read_workbook(&src).unwrap();
    let left = &sheets.iter().find(|(name, _)| name == "left").unwrap().1;
    let right = &sheets.iter().find(|(name, _)| name == "right").unwrap().1;
    let spec = JoinSpec {
        join_type: JoinType::Left,
        left_keys: vec![0],
        right_keys: vec![0],
        left_pick: Some(vec![77, 88]),
        right_pick: vec![1],
        key_mode: KeyMode::NORMALIZE,
        expand_dup: true,
    };
    let result = join(left, right, &spec);
    assert_eq!(result.table.headers, ["部门"]);
    write_joined_xlsx(&result.table, left, right, &out).unwrap();
    // 按工作表绝对坐标验证,越界的 A 列不占输出列,未命中行保持为空。
    assert_eq!(result.table.row_count(), 3);
    let mut workbook = open_workbook_auto(&out).unwrap();
    let actual = workbook.worksheet_range_at(0).unwrap().unwrap();
    assert_eq!(actual.get_size(), (4, 1));
    assert_eq!(actual.get_value((0, 0)), Some(&Data::String("部门".into())));
    assert_eq!(
        actual.get_value((1, 0)),
        Some(&Data::String("工程部".into()))
    );
    assert_eq!(actual.get_value((2, 0)), Some(&Data::Empty));
    assert_eq!(
        actual.get_value((3, 0)),
        Some(&Data::String("产品部".into()))
    );
}
