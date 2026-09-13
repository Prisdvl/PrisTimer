//! CSV 导出命令与相关净化工具。

use tauri::State;

use crate::state::SharedRecorder;

///
/// 前端传来的字符串终究是**外部输入**，`join()` 之前必须过一遍 ——
/// 否则一个 `..\..\Windows\System32\x.csv` 就能写到任意位置。
fn check_csv_filename(filename: &str) -> Result<(), String> {
    if filename.is_empty()
        || !filename
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err("非法文件名".into());
    }
    Ok(())
}

/// `YYYY-MM-DD` 形状校验。不引正则，逐位判断即可。
///
/// 参数化查询本身是安全的，但一个畸形日期进 `BETWEEN` 只会**静默**返回空报告 ——
/// 用户拿到一份全是零的周报却不知道哪里错了。宁可当场报错。
fn is_iso_day(day: &str) -> bool {
    let bytes = day.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
}

/// 毫秒 → 「X 小时 Y 分」/「Y 分钟」的人类可读文本（与统计页同口径）。
fn hm_text(ms: i64) -> String {
    let total_min = (ms as f64 / 60_000.0).round() as i64;
    if total_min >= 60 {
        let (h, m) = (total_min / 60, total_min % 60);
        if m == 0 {
            format!("{h} 小时")
        } else {
            format!("{h} 小时 {m} 分")
        }
    } else {
        format!("{total_min} 分钟")
    }
}

/// CSV 转义：含分隔符/引号/换行的字段用引号包起来，内部引号翻倍。
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 下载目录（`%USERPROFILE%\Downloads`）。两个导出命令共用。
fn downloads_dir() -> Result<std::path::PathBuf, String> {
    std::env::var("USERPROFILE")
        .map(|home| std::path::PathBuf::from(home).join("Downloads"))
        .ok()
        .filter(|dir| dir.is_dir())
        .ok_or_else(|| "找不到下载目录".to_string())
}

/// 导出全部已结算会话为 CSV（UTF-8 BOM，Excel 可直接打开），写入系统
/// 下载目录，返回完整路径。
///
/// 时间戳在 SQL 层用 `datetime(..., 'localtime')` 转成本地时刻 —— 与统计
/// 页的本地日口径一致，也避免为格式化一个文件名引入 chrono。
#[tauri::command]
pub(crate) fn export_csv(
    filename: String,
    recorder: State<'_, SharedRecorder>,
) -> Result<String, String> {
    check_csv_filename(&filename)?;

    let rows = {
        let guard = recorder.lock().map_err(|err| err.to_string())?;
        let conn = guard.store().conn();
        let mut stmt = conn
            .prepare(
                "SELECT datetime(started_at / 1000, 'unixepoch', 'localtime'),
                        kind, elapsed_ms, completed, IFNULL(tag, ''), IFNULL(note, '')
                 FROM session
                 WHERE state = 'finished' AND ended_at IS NOT NULL
                 ORDER BY started_at",
            )
            .map_err(|err| err.to_string())?;
        // 不直接命名 rusqlite 的类型（本 crate 未依赖它），逐行转换靠推断。
        let mut rows: Vec<(String, String, i64, i64, String, String)> = Vec::new();
        for row in stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|err| err.to_string())?
        {
            rows.push(row.map_err(|err| err.to_string())?);
        }
        rows
    };

    let kind_label = |raw: &str| match raw {
        "countdown" => "倒计时",
        _ => "正计时",
    };

    let mut csv = String::from("\u{FEFF}开始时间,类型,时长(分钟),到点完成,标签,备注\r\n");
    for (started, kind, elapsed_ms, completed, tag, note) in rows {
        csv.push_str(&csv_field(&started));
        csv.push(',');
        csv.push_str(kind_label(&kind));
        csv.push(',');
        csv.push_str(&format!("{:.1}", elapsed_ms as f64 / 60_000.0));
        csv.push(',');
        csv.push_str(if completed != 0 { "是" } else { "否" });
        csv.push(',');
        csv.push_str(&csv_field(&tag));
        csv.push(',');
        csv.push_str(&csv_field(&note));
        csv.push_str("\r\n");
    }

    let path = downloads_dir()?.join(&filename);
    std::fs::write(&path, csv.as_bytes()).map_err(|err| err.to_string())?;
    Ok(path.display().to_string())
}

/// 导出「周报」CSV：一份人类可读的三段式报告，写入下载目录并返回完整路径。
///
/// 结构（每段之间空一行，Excel 打开层次分明）：
/// 1. 区间汇总 —— 范围 / 总时长 / 会话数 / 日均
/// 2. 科目汇总 —— 科目 / 总分钟 / 会话数 / 平均每次 / 占比
/// 3. 每日明细 —— 日期 / 总分钟 / 会话数
///
/// 数据源是 `daily_stats` 与 `tag_totals_between`，**与统计页逐位同源** ——
/// 报告里的数字必定等于屏幕上看到的，不会出现「导出后对不上」的尴尬。
/// 总数从每日明细**累加**而非另发一次 SUM 查询，保证三段之间的勾稽关系
/// 天然成立（不会因两次查询之间的写入而错位）。
#[tauri::command]
pub(crate) fn export_report_csv(
    filename: String,
    from_day: String,
    to_day: String,
    recorder: State<'_, SharedRecorder>,
) -> Result<String, String> {
    check_csv_filename(&filename)?;
    for day in [&from_day, &to_day] {
        if !is_iso_day(day) {
            return Err(format!("日期格式应为 YYYY-MM-DD：{day}"));
        }
    }

    let (daily, tags) = {
        let guard = recorder.lock().map_err(|err| err.to_string())?;
        let store = guard.store();
        (
            store
                .daily_stats(&from_day, &to_day)
                .map_err(|err| err.to_string())?,
            store
                .tag_totals_between(&from_day, &to_day)
                .map_err(|err| err.to_string())?,
        )
    };

    let total_ms: i64 = daily.iter().map(|d| d.total_ms).sum();
    let session_count: i64 = daily.iter().map(|d| d.session_count).sum();

    let mut csv = String::from("\u{FEFF}");
    csv.push_str("PrisTimer 专注周报\r\n");
    csv.push_str(&format!("统计范围,{from_day} 至 {to_day}\r\n"));
    csv.push_str(&format!("总专注时长,{}\r\n", hm_text(total_ms)));
    csv.push_str(&format!("总会话数,{session_count}\r\n"));
    // 日均按「有记录的天数」算 —— 空白天不该拉低平均值，口径与统计页一致。
    if !daily.is_empty() {
        csv.push_str(&format!(
            "日均专注,{}\r\n",
            hm_text(total_ms / daily.len() as i64)
        ));
    }

    csv.push_str("\r\n【科目汇总】\r\n");
    csv.push_str("科目,总分钟,会话数,平均每次(分钟),占比\r\n");
    if tags.is_empty() {
        csv.push_str("（该区间没有记录）\r\n");
    }
    for t in &tags {
        let name = if t.tag.is_empty() { "未标注" } else { t.tag.as_str() };
        let minutes = t.total_ms as f64 / 60_000.0;
        let avg = if t.session_count > 0 {
            minutes / t.session_count as f64
        } else {
            0.0
        };
        let percent = if total_ms > 0 {
            t.total_ms as f64 * 100.0 / total_ms as f64
        } else {
            0.0
        };
        csv.push_str(&format!(
            "{},{:.1},{},{:.1},{:.1}%\r\n",
            csv_field(name),
            minutes,
            t.session_count,
            avg,
            percent
        ));
    }

    csv.push_str("\r\n【每日明细】\r\n");
    csv.push_str("日期,总分钟,会话数\r\n");
    if daily.is_empty() {
        csv.push_str("（该区间没有记录）\r\n");
    }
    for d in &daily {
        csv.push_str(&format!(
            "{},{:.1},{}\r\n",
            csv_field(&d.day),
            d.total_ms as f64 / 60_000.0,
            d.session_count
        ));
    }

    let path = downloads_dir()?.join(&filename);
    std::fs::write(&path, csv.as_bytes()).map_err(|err| err.to_string())?;
    Ok(path.display().to_string())
}
