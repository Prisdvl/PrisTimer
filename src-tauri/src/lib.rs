use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::Sender;

use pristimer_core::{
    Command, Phase, PomodoroConfig, SystemClock, TimerRuntime, TimerSnapshot, TimerState,
};
use pristimer_store::{
    DailyStat, HourTotal, Recovered, Store, TagDayTotal, TagTotal,
};
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_notification::NotificationExt;

mod log;
mod recorder;
mod pomodoro;
use pomodoro::{PomodoroManager, PomodoroStatus};
use recorder::Recorder;

/// 取锁，**中毒也能继续用**。
///
/// 默认的 `.lock().unwrap()` 把「某个持锁线程曾经 panic 过」升级成「之后每一次
/// 访问都 panic」。对一个 release GUI 应用来说这没有意义：状态里躺着的是一份
/// 可能略有偏差的几何 / 计时数据，而代价却是整个窗口消失。
/// 取出内部数据继续用，把错误降级成"这一帧可能不准"，然后再靠日志去定位。
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 计时运行时的全局句柄。
struct AppTimer(Mutex<TimerRuntime>);

/// 记录器句柄：既要给计时线程的快照回调用，也要给统计命令用，
/// 因此用 `Arc` 让两边共享同一个实例（同一个数据库连接）。
type SharedRecorder = Arc<Mutex<Recorder>>;

/// 番茄钟编排器的全局句柄。快照回调与命令两边共享。
type SharedPomodoro = Arc<Mutex<PomodoroManager>>;

/// 最新一帧快照的缓存。
///
/// 事件是推送式的：前端 `listen` 订阅完成之前发出的快照不会补发，
/// 而计时线程在 `setup()` 阶段就发出了第一帧（恢复出来的暂停态）。
/// 所以前端挂载后需要主动拉一次当前值，这个缓存就是那次拉取的数据源。
type SnapshotCache = Arc<Mutex<Option<TimerSnapshot>>>;

/// 给快照回调反向下发命令用的发送端。
///
/// 回调在工作线程上跑，不能经 `AppTimer` 的互斥锁绕回来（线程等自己），
/// 所以另开一个 `Mutex<Option<Sender>>`，setup 完成后立刻填上。
/// 番茄模式在 setup 期间不可能已开启，因此「回调早于填充」的窗口是安全的。
type CommandCell = Arc<Mutex<Option<Sender<Command>>>>;

/// 统计总览。
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsSummary {
    total_ms: i64,
    session_count: i64,
}

#[tauri::command]
fn timer_start(state: State<'_, AppTimer>) {
    lock(&state.0).send(Command::Start);
}

#[tauri::command]
fn timer_pause(state: State<'_, AppTimer>) {
    lock(&state.0).send(Command::Pause);
}

#[tauri::command]
fn timer_reset(state: State<'_, AppTimer>) {
    lock(&state.0).send(Command::Reset);
}

#[tauri::command]
fn timer_set_limit(limit_ms: Option<u64>, state: State<'_, AppTimer>) {
    lock(&state.0).send(Command::SetLimit(limit_ms));
}

/// 开关番茄模式。返回切换后的状态，前端也可走事件通道。
#[tauri::command]
fn pomodoro_set(
    enabled: bool,
    pomodoro: State<'_, SharedPomodoro>,
    recorder: State<'_, SharedRecorder>,
    timer: State<'_, AppTimer>,
) -> PomodoroStatus {
    let (status, record, armed_ms) = {
        let mut guard = lock(&pomodoro);
        let status = if enabled {
            guard.enable()
        } else {
            guard.disable()
        };
        (status, guard.should_record(), guard.armed_duration())
    };
    // 专注落库、休息跳过 —— 与回调里的推进逻辑共用同一个开关。
    if let Ok(mut rec) = recorder.lock() {
        rec.set_recording(record);
    }
    if enabled {
        // 武装引擎：回 Idle、把上限设为专注时长，表盘立即显示 25:00。
        // 用户按下「开始」后跑的就是第一个番茄。
        let runtime = lock(&timer.0);
        runtime.send(Command::Reset);
        runtime.send(Command::SetLimit(Some(armed_ms)));
    }
    status
}

/// 当前番茄钟状态。前端挂载时主动拉一次（事件不补发，与 timer_current 同理）。
#[tauri::command]
fn pomodoro_current(pomodoro: State<'_, SharedPomodoro>) -> PomodoroStatus {
    lock(&pomodoro).status()
}

/// 当前番茄钟配置（前端设置面板的初始值）。
#[tauri::command]
fn pomodoro_config_get(pomodoro: State<'_, SharedPomodoro>) -> PomodoroConfig {
    lock(&pomodoro).config()
}

/// 更新番茄钟配置：校验 → 落库 → 热应用。
/// 若模式开启且引擎正停在 Idle（表盘未在计时），立即按新专注时长重武装。
#[tauri::command]
fn pomodoro_config_set(
    config: PomodoroConfig,
    pomodoro: State<'_, SharedPomodoro>,
    recorder: State<'_, SharedRecorder>,
    cache: State<'_, SnapshotCache>,
    timer: State<'_, AppTimer>,
) -> Result<PomodoroStatus, String> {
    config
        .validate()
        .map_err(|field| format!("配置越界: {field}"))?;

    // 先落库（崩溃/重启后设置仍在），再改内存。
    {
        let guard = recorder.lock().map_err(|err| err.to_string())?;
        let json = serde_json::to_string(&config).map_err(|err| err.to_string())?;
        guard
            .store()
            .set_setting("pomodoro_config", &json)
            .map_err(|err| err.to_string())?;
    }

    let status = lock(&pomodoro).apply_config(config);

    // 空闲且开着番茄模式 → 表盘立刻按新专注时长重武装；
    // 正在计时/暂停的阶段不动，跑完后自然用新配置。
    let idle = cache
        .lock()
        .ok()
        .and_then(|c| c.as_ref().map(|s| s.state == TimerState::Idle))
        .unwrap_or(false);
    if idle && status.enabled {
        let runtime = lock(&timer.0);
        runtime.send(Command::Reset);
        runtime.send(Command::SetLimit(Some(config.focus_ms)));
    }
    Ok(status)
}

/// 启动时发现的可接续会话。前端用它决定是否显示提示条。
#[tauri::command]
fn recovered_session(recorder: State<'_, SharedRecorder>) -> Option<Recovered> {
    let guard = recorder.lock().ok()?;
    guard.recovery().cloned()
}

/// 当前计时快照。前端挂载时主动拉取一次，弥补订阅前丢失的推送。
#[tauri::command]
fn timer_current(cache: State<'_, SnapshotCache>) -> Option<TimerSnapshot> {
    cache.lock().ok()?.clone()
}

/// 区间统计：某个日期范围内每天的聚合数据，日历与热力图直接消费。
/// 日期格式 `YYYY-MM-DD`（本地时区），区间含两端。
#[tauri::command]
fn stats_daily(
    from_day: String,
    to_day: String,
    recorder: State<'_, SharedRecorder>,
) -> Result<Vec<DailyStat>, String> {
    let guard = recorder.lock().map_err(|err| err.to_string())?;
    guard
        .store()
        .daily_stats(&from_day, &to_day)
        .map_err(|err| err.to_string())
}

/// 累计总览。
#[tauri::command]
fn stats_summary(recorder: State<'_, SharedRecorder>) -> Result<StatsSummary, String> {
    let guard = recorder.lock().map_err(|err| err.to_string())?;
    let store = guard.store();
    Ok(StatsSummary {
        total_ms: store.total_ms().map_err(|err| err.to_string())?,
        session_count: store.finished_count().map_err(|err| err.to_string())?,
    })
}

/// 文件名白名单：只放行 ASCII 字母数字与 `_-.`，杜绝路径穿越与非法字符。
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
fn export_csv(
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
fn export_report_csv(
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

/// 按标签聚合的专注时长（科目统计）。日期范围含两端，与 stats_daily 同口径。
#[tauri::command]
fn stats_tags(
    from_day: String,
    to_day: String,
    recorder: State<'_, SharedRecorder>,
) -> Result<Vec<TagTotal>, String> {
    let guard = recorder.lock().map_err(|err| err.to_string())?;
    guard
        .store()
        .tag_totals_between(&from_day, &to_day)
        .map_err(|err| err.to_string())
}

/// 区间内按「本地日 × 标签」聚合的专注时长（近 7 天回顾的目标达成天数消费）。
/// 日期格式 `YYYY-MM-DD`（本地时区），区间含两端。
#[tauri::command]
fn stats_tag_daily(
    from_day: String,
    to_day: String,
    recorder: State<'_, SharedRecorder>,
) -> Result<Vec<TagDayTotal>, String> {
    let guard = recorder.lock().map_err(|err| err.to_string())?;
    guard
        .store()
        .tag_daily_totals_between(&from_day, &to_day)
        .map_err(|err| err.to_string())
}

/// 区间内按「会话开始时刻的本地小时」（0–23）聚合的专注时长。
/// 日期格式 `YYYY-MM-DD`（本地时区），区间含两端。
#[tauri::command]
fn stats_hourly(
    from_day: String,
    to_day: String,
    recorder: State<'_, SharedRecorder>,
) -> Result<Vec<HourTotal>, String> {
    let guard = recorder.lock().map_err(|err| err.to_string())?;
    guard
        .store()
        .hour_totals_between(&from_day, &to_day)
        .map_err(|err| err.to_string())
}

/// 设置下一条会话的标签（科目）。规范化规则：去首尾空白、限 20 字符、
/// 空串视为清除。规范化后的值落库为 `current_tag`，重启后沿用。
#[tauri::command]
fn tag_set(
    tag: Option<String>,
    recorder: State<'_, SharedRecorder>,
) -> Result<Option<String>, String> {
    let normalized = tag
        .map(|t| t.trim().chars().take(20).collect::<String>())
        .filter(|t| !t.is_empty());

    {
        let mut guard = recorder.lock().map_err(|err| err.to_string())?;
        let json = serde_json::to_string(&normalized).map_err(|err| err.to_string())?;
        guard
            .store()
            .set_setting("current_tag", &json)
            .map_err(|err| err.to_string())?;
        guard.set_tag(normalized.clone());
    }
    Ok(normalized)
}

/// 当前待用标签（科目）。前端挂载时拉一次做回显。
#[tauri::command]
fn tag_current(recorder: State<'_, SharedRecorder>) -> Option<String> {
    let guard = recorder.lock().ok()?;
    guard.current_tag().cloned()
}

/// 每日复习目标：科目 → 目标分钟数，落 `tag_goals` 键（JSON 对象）。
///
/// 校验规则与 `tag_set` 同源：科目去首尾空白、限 20 字；分钟数 1–1440
/// （一天上限）；目标总数最多 8 个。用 BTreeMap 是为了让序列化结果
/// 键序稳定，落库内容可读、可 diff。
#[tauri::command]
fn tag_goals_set(
    goals: BTreeMap<String, u64>,
    recorder: State<'_, SharedRecorder>,
) -> Result<BTreeMap<String, u64>, String> {
    let mut normalized = BTreeMap::new();
    for (tag, minutes) in goals {
        let tag = tag.trim().chars().take(20).collect::<String>();
        if tag.is_empty() {
            return Err("科目名不能为空".into());
        }
        if minutes == 0 || minutes > 1440 {
            return Err(format!("目标分钟数越界: {tag} → {minutes}（应为 1–1440）"));
        }
        normalized.insert(tag, minutes);
    }
    if normalized.len() > 8 {
        return Err("目标最多 8 个科目".into());
    }

    let json = serde_json::to_string(&normalized).map_err(|err| err.to_string())?;
    let guard = recorder.lock().map_err(|err| err.to_string())?;
    guard
        .store()
        .set_setting("tag_goals", &json)
        .map_err(|err| err.to_string())?;
    Ok(normalized)
}

/// 读取每日目标。无保存记录或数据损坏时返回空表（坏数据不让统计页挂掉，
/// 与番茄配置「静默回默认」同一策略）。
#[tauri::command]
fn tag_goals_get(
    recorder: State<'_, SharedRecorder>,
) -> Result<BTreeMap<String, u64>, String> {
    let guard = recorder.lock().map_err(|err| err.to_string())?;
    let raw = guard
        .store()
        .get_setting("tag_goals")
        .map_err(|err| err.to_string())?
        .unwrap_or_else(|| "{}".to_string());
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

/// 重命名（或合并、清除）历史会话的标签：所有 `tag = from` 的会话改为
/// `to`，返回改写行数。`to` 传 `None`/空 表示清为未标注。
///
/// 联动修正，避免「改了历史、现状还对不上」：
/// - 当前待用标签等于旧名 → 同步改成新名并落库；
/// - 每日目标含旧科目键 → 迁移到新键（合并到已有目标时保留较大的那份）。
#[tauri::command]
fn tag_rename(
    from: String,
    to: Option<String>,
    recorder: State<'_, SharedRecorder>,
) -> Result<u64, String> {
    let to_normalized = to
        .map(|t| t.trim().chars().take(20).collect::<String>())
        .filter(|t| !t.is_empty());

    let changed = {
        let mut guard = recorder.lock().map_err(|err| err.to_string())?;
        let changed = guard
            .store()
            .rename_tag(&from, to_normalized.as_deref())
            .map_err(|err| err.to_string())?;

        // 待用标签联动：正打算用旧标签打下一枪的话，换成新名。
        if let Some(current) = guard.current_tag() {
            if *current == from {
                guard.set_tag(to_normalized.clone());
                let json = serde_json::to_string(&to_normalized).map_err(|err| err.to_string())?;
                guard
                    .store()
                    .set_setting("current_tag", &json)
                    .map_err(|err| err.to_string())?;
            }
        }

        // 每日目标联动：旧键迁到新键；合并撞键时保留较大的目标。
        if changed > 0 {
            let raw = guard
                .store()
                .get_setting("tag_goals")
                .map_err(|err| err.to_string())?
                .unwrap_or_else(|| "{}".to_string());
            let mut goals: BTreeMap<String, u64> =
                serde_json::from_str(&raw).unwrap_or_default();
            if let Some(minutes) = goals.remove(&from) {
                match to_normalized.as_deref() {
                    Some(new_tag) => {
                        goals
                            .entry(new_tag.to_string())
                            .and_modify(|old| *old = (*old).max(minutes))
                            .or_insert(minutes);
                    }
                    None => {
                        // 清为未标注：目标跟着作废（没有「未标注」的目标语义）。
                    }
                }
                let json = serde_json::to_string(&goals).map_err(|err| err.to_string())?;
                guard
                    .store()
                    .set_setting("tag_goals", &json)
                    .map_err(|err| err.to_string())?;
            }
        }
        changed as u64
    };
    Ok(changed)
}

/// 从托盘恢复主窗口。
fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

// ---------------------------------------------------------------------------
// 窗口几何持久化
//
// 为什么放在 Rust 而不是前端 localStorage：
//   窗口必须「以记忆中的几何直接出现」，而不是先露出默认位置再跳过去。
//   前端要等 WebView2 把页面加载完（实测 7 秒上下，`visible: false` 期间还会
//   被节流拖到 13 秒）才拿得到 localStorage 里的值 —— 那时窗口早被用户看见了。
//   Rust 在 `setup()` 里同步读一个小 JSON、改完几何再 `show()`，几十毫秒完事。
//
// 坐标系（实测，极易踩）：
//   · `set_size` / `inner_size`      = 客户区尺寸
//   · `set_position` / `outer_position` = 外框左上角
//   存和取必须用同一对；混用（存 outer_size 却用 set_size 还原）会让窗口
//   每次重启长大一圈边框 —— 16×9，肉眼看不出来，重启十次就少一块内容。
// ---------------------------------------------------------------------------

/// 迷你组件的客户区尺寸（物理像素 = 该值 × 缩放因子）。
const MINI_W: f64 = 264.0;
const MINI_H: f64 = 96.0;
/// 常规窗口的最小尺寸。不写进 tauri.conf —— 静态最小尺寸会把 264px 的
/// 迷你小窗挡在门外，所以只在常规形态下运行时设置。
const MIN_W: f64 = 760.0;
const MIN_H: f64 = 560.0;

/// 常规形态的几何：客户区尺寸 + 外框左上角。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NormalGeom {
    w: f64,
    h: f64,
    x: i32,
    y: i32,
}

/// 迷你组件的位置（外框左上角）。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MiniPos {
    x: i32,
    y: i32,
}

/// 落盘的窗口状态。字段全可选：老版本文件、手改坏的文件都不该让启动失败。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WinStateFile {
    #[serde(default)]
    normal: Option<NormalGeom>,
    #[serde(default)]
    maximized: bool,
    #[serde(default)]
    mini_mode: bool,
    #[serde(default)]
    mini: Option<MiniPos>,
}

fn win_state_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("window.json"))
}

/// 读窗口状态。文件不存在、损坏、字段缺失一律回默认值 —— 宁可用默认几何，
/// 也不能因为一个坏文件就启动不了。
fn read_win_state(app: &AppHandle) -> WinStateFile {
    win_state_path(app)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_win_state(app: &AppHandle, state: &WinStateFile) -> Result<(), String> {
    let path = win_state_path(app).ok_or_else(|| "找不到应用数据目录".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    let json = serde_json::to_string_pretty(state).map_err(|err| err.to_string())?;
    std::fs::write(&path, json).map_err(|err| err.to_string())
}

/// 位置是否落在某台显示器的可见范围内（留 40px 容差）。
/// 拔掉显示器、改过分辨率之后，存下的位置可能已经跑到屏幕外 —— 那就别恢复。
fn position_on_screen(win: &tauri::WebviewWindow, x: i32, y: i32, w: i32, h: i32) -> bool {
    let Ok(monitors) = win.available_monitors() else {
        return false;
    };
    monitors.iter().any(|m| {
        let pos = m.position();
        let size = m.size();
        let (mw, mh) = (size.width as i32, size.height as i32);
        x + w > pos.x + 40 && x < pos.x + mw - 40 && y + h > pos.y + 40 && y < pos.y + mh - 40
    })
}

/// 给无边框窗口补上 DWM 圆角。
///
/// `decorations: false` 的窗口没有 `WS_CAPTION`，Windows **不会**给它自动圆角 ——
/// 从系统角度看它就是个矩形，四个直角在深色桌面上显得很生硬（尤其迷你组件那种
/// 小尺寸窗口，直角几乎占满了视觉重量）。
///
/// `DWMWA_WINDOW_CORNER_PREFERENCE` 是 Win11（Build 22000+）才支持的属性，
/// 更老的系统会直接返回失败 —— 忽略即可，退回直角不影响任何功能。
#[cfg(windows)]
fn apply_round_corners(win: &tauri::WebviewWindow, _small: bool) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    let Ok(hwnd) = win.hwnd() else {
        return;
    };
    // 迷你组件与常规窗口统一用标准半径（DWMWCP_ROUND ≈ 8px）——
    // 之前小组件用 ROUNDSMALL（≈4px），在 264×96 的窗口上圆感几乎不可见，
    // 用户明确要求迷你组件也是圆角。
    let preference: i32 = DWMWCP_ROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd.0,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &preference as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

#[cfg(not(windows))]
fn apply_round_corners(_win: &tauri::WebviewWindow, _small: bool) {}

/// 按记忆的几何摆好窗口，然后 `show()` 出来。
///
/// 必须在窗口被用户看见之前调用，且这是**唯一**的启动期几何来源 ——
/// 前端不再参与恢复，也就没有「先默认位置、再跳过去」这一出。
fn place_main_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let state = read_win_state(app);
    apply_round_corners(&win, state.mini_mode);

    if state.mini_mode {
        // 迷你组件：固定客户区尺寸 + 置顶 + 贴边定位 + 关阴影（不可见 frame 黑框）
        let scale = win.scale_factor().unwrap_or(1.0);
        let _ = win.set_shadow(false);
        let pw = (MINI_W * scale).round() as u32;
        let ph = (MINI_H * scale).round() as u32;
        let _ = win.set_resizable(false);
        let _ = win.set_min_size(None::<tauri::LogicalSize<f64>>);
        let _ = win.set_size(tauri::PhysicalSize::new(pw, ph));
        let _ = win.set_always_on_top(true);

        // 位置是外框左上角，贴边就得用外框尺寸 —— 拿客户区宽度去算，
        // 组件右边缘会探出屏幕（264 + 14 的边距配 280 的外框，正好越界 2px）。
        let (ow, oh) = win
            .outer_size()
            .map(|s| (s.width as i32, s.height as i32))
            .unwrap_or((pw as i32, ph as i32));
        let placed = state
            .mini
            .filter(|m| position_on_screen(&win, m.x, m.y, ow, oh))
            .map(|m| {
                let _ = win.set_position(tauri::PhysicalPosition::new(m.x, m.y));
            })
            .is_some();
        if !placed {
            if let Ok(Some(monitor)) = win.current_monitor() {
                let margin = (14.0 * scale).round() as i32;
                let mpos = monitor.position();
                let msize = monitor.size();
                let _ = win.set_position(tauri::PhysicalPosition::new(
                    mpos.x + msize.width as i32 - ow - margin,
                    mpos.y + msize.height as i32 - oh - margin,
                ));
            }
        }
    } else {
        let _ = win.set_shadow(true);
        let _ = win.set_resizable(true);
        let _ = win.set_min_size(Some(tauri::LogicalSize::new(MIN_W, MIN_H)));
        if let Some(n) = state.normal {
            if n.w >= 400.0
                && n.h >= 300.0
                && position_on_screen(&win, n.x, n.y, n.w as i32, n.h as i32)
            {
                let _ = win.set_size(tauri::PhysicalSize::new(n.w as u32, n.h as u32));
                let _ = win.set_position(tauri::PhysicalPosition::new(n.x, n.y));
            }
        }
        if state.maximized {
            let _ = win.maximize();
        }
    }

    let _ = win.show();
    let _ = win.set_focus();
}

/// 合并式写入窗口状态：只更新传进来的字段，其余保留。
///
/// 合并而不是整体覆盖，是因为调用方各自只知道自己那一部分 —— 移动窗口时
/// 只知道常规几何，切迷你时只知道形态标志，整体覆盖会把对方的信息抹掉。
#[tauri::command]
fn win_state_save(
    app: AppHandle,
    normal: Option<NormalGeom>,
    maximized: Option<bool>,
    mini_mode: Option<bool>,
    mini: Option<MiniPos>,
) -> Result<WinStateFile, String> {
    let mut state = read_win_state(&app);
    if let Some(n) = normal {
        state.normal = Some(n);
    }
    if let Some(m) = maximized {
        state.maximized = m;
    }
    if let Some(b) = mini_mode {
        state.mini_mode = b;
        // 圆角半径跟着形态走：小组件用 ROUNDSMALL，常规窗口用 ROUND。
        if let Some(win) = app.get_webview_window("main") {
            apply_round_corners(&win, b);
        }
    }
    if let Some(p) = mini {
        state.mini = Some(p);
    }
    write_win_state(&app, &state)?;
    Ok(state)
}

/// 读当前窗口状态（前端挂载时对齐用）。
#[tauri::command]
fn win_state_get(app: AppHandle) -> WinStateFile {
    read_win_state(&app)
}

/// 迷你形态的窗口属性开关：一次 IPC 完成 4 项设置。
///
/// 之前前端逐项 set_resizable / set_min_size / set_always_on_top / set_shadow，
/// 每次实测 ~21ms，4 次串行在动画关键路径上白占 ~80ms（内容已淡出、窗口干等）。
#[tauri::command]
async fn set_mini_shell(app: AppHandle, mini: bool) -> Result<(), String> {
    use tauri::Manager;
    let win = app.get_webview_window("main").ok_or("主窗口不存在")?;
    if mini {
        win.set_resizable(false).map_err(|e| e.to_string())?;
        win.set_min_size::<tauri::LogicalSize<f64>>(None)
            .map_err(|e| e.to_string())?;
        win.set_always_on_top(true).map_err(|e| e.to_string())?;
        // ★ 迷你态关 DWM 阴影：shadow=true 的不可见 resize frame 是"黑框"来源
        win.set_shadow(false).map_err(|e| e.to_string())?;
    } else {
        win.set_always_on_top(false).map_err(|e| e.to_string())?;
        win.set_shadow(true).map_err(|e| e.to_string())?;
        win.set_min_size(Some(tauri::LogicalSize::new(760.0, 560.0)))
            .map_err(|e| e.to_string())?;
        win.set_resizable(true).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 原生窗口几何动画：把"迷你↔常规"的过渡从 JS rAF 挪到 Rust 侧分帧执行。
///
/// 为什么必须挪：旧实现每帧调 `setSize` + `setPosition` 两次 IPC，WebView2 桥
/// 单程 2–6ms、两次再加 await 排队，一帧 60fps 的 16ms 预算被吃光 —— 观感卡顿。
/// 这里每帧只是一次本地 Win32 调用（微秒级），节奏由 `Instant` 驱动，与调用方
/// 帧率解耦；曲线仍是 easeOutCubic，观感一致。
///
/// 尺寸语义与前端 `set_size` 对齐：参数是**客户区**物理尺寸；无边框窗口的
/// 客户区原点即外框原点，外框比客户区多出的部分（shadow=true 时的不可见
/// resize frame）全部落在右侧/下方，按帧把差值加回外框即可。
#[tauri::command]
async fn animate_window_to(
    app: AppHandle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    duration_ms: u64,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::time::{Duration, Instant};
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::Graphics::Dwm::DwmFlush;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetClientRect, GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
        };
        let win = app.get_webview_window("main").ok_or("主窗口不存在")?;
        // HWND 裸指针不能跨线程搬，转成数值再在阻塞线程里还原
        let hwnd_val = win.hwnd().map_err(|e| e.to_string())?.0 as isize;
        tauri::async_runtime::spawn_blocking(move || {
            let hwnd = hwnd_val as *mut core::ffi::c_void;
            // SAFETY：hwnd 是本应用主窗口的有效句柄（单实例守卫保证唯一）；
            // GetClientRect/GetWindowRect/SetWindowPos 都是纯查询/几何操作。
            let mut cr = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let mut wr = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if unsafe { GetClientRect(hwnd, &mut cr) } == 0
                || unsafe { GetWindowRect(hwnd, &mut wr) } == 0
            {
                return Err("GetClientRect/GetWindowRect 失败".into());
            }
            let from_w = (cr.right - cr.left) as f64;
            let from_h = (cr.bottom - cr.top) as f64;
            let from_x = wr.left as f64;
            let from_y = wr.top as f64;
            // 外框 − 客户区 的差值（无边框窗全部在右侧/下方）
            let dw = (wr.right - wr.left - (cr.right - cr.left)) as f64;
            let dh = (wr.bottom - wr.top - (cr.bottom - cr.top)) as f64;
                let dur = (duration_ms.max(1) as f64) / 1000.0;
                let t0 = Instant::now();
                loop {
                    let step_start = Instant::now();
                    let t = ((step_start - t0).as_secs_f64() / dur).min(1.0);
                    let e = 1.0 - (1.0 - t).powi(3);
                    let cw = from_w + (w as f64 - from_w) * e;
                    let ch = from_h + (h as f64 - from_h) * e;
                    let ox = from_x + (x as f64 - from_x) * e;
                    let oy = from_y + (y as f64 - from_y) * e;
                    unsafe {
                        SetWindowPos(
                            hwnd,
                            std::ptr::null_mut(),
                            ox.round() as i32,
                            oy.round() as i32,
                            (cw + dw).round() as i32,
                            (ch + dh).round() as i32,
                            SWP_NOACTIVATE | SWP_NOZORDER,
                        );
                    }
                    if t >= 1.0 {
                        break;
                    }
                    // ★ 双重节流：最小步进 15ms（≤67fps）+ DwmFlush 对齐合成器 vsync。
                    //   每次 resize 对 WebView2 都是一次完整的布局+合成事务，实测
                    //   1ms 步进（~200Hz）会把消息泵淹没 —— 页面 rAF 停摆数百 ms、
                    //   尾部多步被合并成跳变（观感就是"卡顿"）。60fps 节流后全动画
                    //   只有 ~19 次事务，间隔充裕，渲染管线零积压。
                    let budget = Duration::from_millis(15);
                    let spent = step_start.elapsed();
                    if spent < budget {
                        std::thread::sleep(budget - spent);
                    }
                    unsafe {
                        let _ = DwmFlush();
                    }
                }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (x, y, w, h);
        use tauri::Manager;
        let win = app.get_webview_window("main").ok_or("主窗口不存在")?;
        let scale = win.scale_factor().map_err(|e| e.to_string())?;
        let _ = scale;
        win.set_size(tauri::PhysicalSize::new(w as u32, h as u32))
            .map_err(|e| e.to_string())?;
        win.set_position(tauri::PhysicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
        let _ = duration_ms;
        Ok(())
    }
}

/// 番茄阶段到点后给用户的通知文案。
fn pomodoro_notice(phase: Phase, duration_ms: u64) -> (String, String) {
    let minutes = duration_ms / 60_000;
    match phase {
        Phase::ShortBreak => (
            "专注完成".into(),
            format!("休息 {minutes} 分钟，站起来走走"),
        ),
        Phase::LongBreak => (
            "连续专注达成".into(),
            format!("来一次 {minutes} 分钟的长休息"),
        ),
        Phase::Focus => (
            "休息结束".into(),
            "新的番茄开始了，进入专注".into(),
        ),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();

    // ★ 日志必须在**任何** Tauri 初始化之前就位。
    //
    // `setup()` 里那句 `log::init` 已经太晚：`App::build` 阶段自己就可能失败
    // （WebView2 环境建不起来、运行时被策略禁用、资源目录不可写）。那条路径上
    // 我们连"原因"都写不出去 —— 而这正是「图标点了，闪一下就没了，日志里
    // 干干净净」的成因之一。用上下文里的 identifier 自己拼出与
    // `app_data_dir()` 一致的位置（Windows 即 `%APPDATA%\<identifier>`）。
    if let Some(dir) = early_data_dir(&context) {
        log::init(&dir);
    }

    // 单实例守卫：必须在 `build_app().run()` 之前 —— Tauri 一旦开始建窗口、
    // 跑 setup，再想「不启动」就已经太晚了（setup 里会去开数据库，而那正是
    // 旧实例锁着的东西）。
    //
    // 但它**不该**比日志更早。守卫自己会写几行关键的日志（"已有实例在运行，
    // 已把它的窗口带到前台" / "窗口尚未就绪"），放在 `log::init` 之前，这些行
    // 会因为日志句柄尚未建立而**静默丢失** —— 而这恰好是排查「双击图标没
    // 反应」时最需要的那几行。`generate_context!` 与 `log::init` 都只碰编译期
    // 内嵌的资源与一个日志文件，不会去动数据库，所以把守卫挪到它们之后是安全的。
    if !acquire_single_instance() {
        return;
    }

    log::log(format!(
        "进程启动 v{} · pid {}",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    ));

    // ★ 兜底：把「进程凭空消失」换成一个日志行 + 一个对话框。
    //
    // Tauri 对 setup 钩子返回 Err 的处理是直接 panic（`app.rs` 里那句
    // "Failed to setup app"）—— 而 GUI 子系统下 panic 信息写向不存在的 stderr，
    // 用户看到的就是「图标点了，什么都没发生」。真实现场是一次
    // `数据库错误: database is locked`：另一个实例正持着写锁，
    // 于是后起的这个在 setup 里失败、进程消失。
    //
    // 触发源已在两处堵住（单实例守卫 + `Store::open` 的重试），这里再兜一层：
    // 今后无论启动阶段因为什么炸掉，用户至少能得到一句人话、我们至少有一行日志。
    // `AssertUnwindSafe` 是必要的：`Context` 不保证 `UnwindSafe`，而我们在这里
    // 明确接受"捕获后立刻退出、不复用任何状态"。
    let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        build_app().run(context)
    }));
    match started {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            log::log(format!("应用异常退出: {err}"));
            show_fatal_dialog(&format!("{err}\n\n{}", where_to_look()));
            std::process::exit(1);
        }
        Err(_) => {
            // panic 的位置与载荷已经由 log.rs 的 panic hook 记下，这里只负责
            // 把它翻译成用户能理解的一句话。
            log::log("启动过程中发生 panic，正在退出");
            show_fatal_dialog(&format!(
                "应用在启动过程中发生内部错误，已退出。\n\n{}",
                where_to_look()
            ));
            std::process::exit(1);
        }
    }
}

/// 推断应用数据目录（与 Tauri 的 `app_data_dir()` 同一处）。
///
/// Windows 上 `app_data_dir()` 就是 `%APPDATA%\<identifier>`；这里提前算一次，
/// 是为了能在 Tauri 还没建起来的时候就把日志打开。
#[cfg(windows)]
fn early_data_dir(context: &tauri::Context) -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(std::path::PathBuf::from(base).join(&context.config().identifier))
}

#[cfg(not(windows))]
fn early_data_dir(_context: &tauri::Context) -> Option<std::path::PathBuf> {
    None
}

/// 告诉用户「去哪儿找原因」。日志还没建立时（问题发生得比 `log::init` 更早）
/// 也如实说，不含糊其辞。
fn where_to_look() -> String {
    match log::path() {
        Some(path) => format!("详细原因见日志：\n{}", path.display()),
        None => "问题发生得太早，日志尚未建立 —— 请检查应用数据目录是否可写。".to_string(),
    }
}

/// 单实例守卫：本进程是否拿到了「唯一实例」的名额。
///
/// 这不只是体面问题。两个实例会各跑一个计时线程、各持一个 SQLite 连接写同一个
/// 库文件：轻则同一条会话被写两遍，重则两个连接同时执行建表 / 切 WAL 时撞锁
/// —— 后起的那个在 setup 钩子里失败，Tauri 直接 panic，**用户在屏幕上看到的
/// 就是"闪一下就没了"**。
///
/// 注意这不是"没设等待"造成的：rusqlite 开连接时已把 `busy_timeout` 设成
/// 5000ms。真正等不来的是 WAL 下的 `SQLITE_BUSY_SNAPSHOT` —— 连接先读
/// pragma 拿到读快照、再想升级为写，而快照已被另一实例的提交作废；SQLite
/// 对这种冲突**刻意不调用忙等处理器**，等 5 秒也一样立刻失败。所以这里必须
/// 从源头不产生第二个实例，而不是指望重试能等过去。
/// 而应用启动要 4–6 秒，等不及再点一次图标是极常见的操作。
///
/// 用的是「进程级命名互斥体」这把最轻的锁：拿到即持有一辈子（不显式释放，
/// 进程退出时由系统回收）。第二个实例发现自己没拿到名额，就把已有窗口捞到
/// 前台然后退出 —— 用户感受到的是「点了图标，窗口出来了」。
#[cfg(windows)]
fn acquire_single_instance() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::{CreateMutexW, Sleep};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };

    let name: Vec<u16> = "Local\\com.pristimer.app.single-instance\0"
        .encode_utf16()
        .collect();
    // SAFETY: 三个调用都是「传一个合法的 UTF-16 以 NUL 结尾的串」这种直白用法；
    // 返回的句柄故意不关 —— 它就是这名额的凭证，关了等于把名额还回去。
    // （windows-sys 0.61 起 `HANDLE` / `HWND` 是裸指针，判空用 `is_null()`。）
    unsafe {
        let guard = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if guard.is_null() {
            // 连互斥体都建不出来（极端受限环境）：不拿它当理由拦住启动
            log::log("单实例互斥体创建失败，按「允许启动」处理");
            return true;
        }
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return true;
        }

        // 已有实例。这里**不能**直接退出走人：本应用启动要 4–6 秒（WebView2
        // 初始化），用户"没看见窗口所以又点了一次"是最常见的重复启动时机 ——
        // 那一刻已有实例的窗口可能还不存在。所以给它一点时间，等窗口出现后
        // 再把它捞到前台，让用户的第二次点击真的"有反应"。
        let title: Vec<u16> = "PrisTimer\0".encode_utf16().collect();
        for _ in 0..40 {
            // 40 × 250ms ≈ 10 秒
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if !hwnd.is_null() {
                // 托盘最小化过就 restore，再置顶（无边框窗口没有 WS_CAPTION，
                // 但 SW_RESTORE/SW_SHOW 一样有效）。
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                log::log("已有实例在运行，已把它的窗口带到前台，本次启动退出");
                return false;
            }
            Sleep(250);
        }
        log::log("已有实例在运行（窗口尚未就绪），本次启动退出");
        false
    }
}

#[cfg(not(windows))]
fn acquire_single_instance() -> bool {
    true
}

/// 把「启动失败」摆到用户眼前。
///
/// 之所以要专门弹窗：这个应用是无边框 + 隐藏控制台的 GUI 进程，它没有任何
/// 可见的失败出口 —— 数据库打不开、磁盘满、权限被拒，用户看到的统一是
/// 「我点了，什么都没发生」。既然已经到了要退出这一步，至少把原因写清楚。
#[cfg(windows)]
fn show_fatal_dialog(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

    let text: Vec<u16> = format!("{message}\0").encode_utf16().collect();
    let caption: Vec<u16> = "PrisTimer 启动失败\0".encode_utf16().collect();
    // SAFETY: 两个参数都是合法的 NUL 结尾 UTF-16 串；hwnd 传空表示无属主窗口。
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_fatal_dialog(message: &str) {
    eprintln!("PrisTimer 启动失败: {message}");
}

fn build_app() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        // 关窗 = 隐藏到托盘，后台继续计时；真正退出走托盘菜单的「退出」。
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .setup(|app| {
            // 1. 打开数据库。放到应用数据目录，各平台会自动定位到合适位置
            //    （Windows: %APPDATA%\com.pristimer.app，macOS: ~/Library/Application Support）。
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            // 日志通常已经在 `run()` 开头就位了（那里才能覆盖 Tauri 自身初始化
            // 失败的路径）。这里再调一次是幂等的兜底 —— 非 Windows 平台拿不到
            // 提前推断的目录，靠这一次打开。
            log::init(&dir);
            log::log(format!("setup 就绪 · 数据目录 {}", dir.display()));
            let db_path = dir.join("pristimer.db");
            // 先记一笔「准备开库」再开：万一 `Store::open` 失败，日志里就能看到
            // 「走到哪一步停的」，而不是只有一条孤零零的 panic。
            log::log(format!(
                "启动 v{} · 数据库 {}",
                env!("CARGO_PKG_VERSION"),
                db_path.display()
            ));
            let store = Store::open(&db_path)?;

            // 番茄配置从 settings 表加载：校验失败（数据被手改坏）就静默回默认值。
            // 必须在 store 移入 Recorder 之前读。
            let saved_config = store
                .get_setting("pomodoro_config")
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_str::<PomodoroConfig>(&raw).ok())
                .filter(|config| config.validate().is_ok());

            // 上次使用的科目标签同样恢复（存的是 JSON，坏数据按无标签处理）。
            let saved_tag = store
                .get_setting("current_tag")
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_str::<Option<String>>(&raw).ok())
                .flatten()
                .filter(|tag| !tag.is_empty());

            // 2. 先做启动恢复。
            let mut recorder = Recorder::new(store);
            let recovered = recorder.recover();
            if saved_tag.is_some() {
                recorder.set_tag(saved_tag.clone());
            }

            let recorder: SharedRecorder = Arc::new(Mutex::new(recorder));
            app.manage(recorder.clone());

            // 3. 番茄钟编排器。先 manage 再进计时回调，闭包里才能克隆到它。
            let pomodoro: SharedPomodoro = Arc::new(Mutex::new(match saved_config {
                Some(config) => PomodoroManager::with_config(config),
                None => PomodoroManager::new(),
            }));
            app.manage(pomodoro.clone());

            // 4. 启动计时线程。恢复状态在这里一次性注入 —— 必须在 spawn 时给出，
            //    而不是 spawn 之后再发命令，否则第一帧会先是 Idle / 0，
            //    记录器会把刚恢复出来的会话当成「用户按了重置」结算掉。
            let handle = app.handle().clone();
            let hook_recorder = recorder.clone();
            let hook_pomodoro = pomodoro.clone();
            let cache: SnapshotCache = Arc::new(Mutex::new(None));
            app.manage(cache.clone());
            let hook_cache = cache.clone();
            // 回调里反向下发命令的发送端，spawn 之后立刻填充。
            let commands: CommandCell = Arc::new(Mutex::new(None));
            let hook_commands = commands.clone();

            let runtime = TimerRuntime::spawn_with(
                SystemClock,
                None,
                recovered.as_ref().map(|r| (r.elapsed_ms, r.limit_ms)),
                move |snapshot: TimerSnapshot| {
                    // 先落库，再推送。锁只持有一次 SQL 的时间，不会影响 100ms 的节拍。
                    if let Ok(mut rec) = hook_recorder.lock() {
                        rec.on_snapshot(snapshot);
                    }
                    // 更新缓存，供前端的 timer_current 命令拉取。
                    if let Ok(mut cached) = hook_cache.lock() {
                        *cached = Some(snapshot);
                    }
                    if let Err(err) = handle.emit("timer:update", snapshot) {
                        log::log(format!("推送计时快照失败: {err}"));
                    }

                    // ---- 番茄循环编排：只在「倒计时自然到点」时推进 ----
                    if snapshot.state == TimerState::Finished {
                        let next = hook_pomodoro
                            .lock()
                            .ok()
                            .filter(|p| p.is_enabled())
                            .map(|mut p| p.advance());
                        if let Some((phase, duration_ms)) = next {
                            // 专注落库、休息跳过：必须在命令入队**之前**切开关，
                            // 因为回调与命令处理在同一线程上串行执行。
                            if let Ok(mut rec) = hook_recorder.lock() {
                                rec.set_recording(phase == Phase::Focus);
                            }
                            if let Ok(guard) = hook_commands.lock() {
                                if let Some(tx) = guard.as_ref() {
                                    // 线程已退出时 send 失败 —— 与 runtime.send
                                    // 同一口径：静默忽略，不让计时线程 panic。
                                    let _ = tx.send(Command::Reset);
                                    let _ = tx.send(Command::SetLimit(Some(duration_ms)));
                                    let _ = tx.send(Command::Start);
                                }
                            }
                            let (title, body) = pomodoro_notice(phase, duration_ms);
                            let _ = handle
                                .notification()
                                .builder()
                                .title(title)
                                .body(body)
                                .show();
                        }
                        let status = hook_pomodoro.lock().ok().map(|p| p.status());
                        if let Some(status) = status {
                            if let Err(err) = handle.emit("pomodoro:update", status) {
                                log::log(format!("推送番茄状态失败: {err}"));
                            }
                        }
                    }
                },
            );

            // 填充回调用的命令发送端（mpsc Sender 可克隆，多生产者入队无锁）。
            *lock(&commands) = Some(runtime.sender());
            app.manage(AppTimer(Mutex::new(runtime)));

            // 5. 窗口是 `visible: false` 启动的：这里按记忆的几何摆好再 show 出来。
            //    放在托盘构建**之前** —— 窗口能否出现只依赖这一步，后面任何环节
            //    （托盘、菜单、系统集成）出问题都不会让它变成"启动了却没有窗口"。
            place_main_window(app.handle());

            // 6. 托盘：左键单击恢复窗口，右键菜单（显示 / 开机自启 / 退出）。
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let autostart_item = CheckMenuItem::with_id(
                app,
                "autostart",
                "开机自启",
                true,
                app.autolaunch().is_enabled().unwrap_or(false),
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &autostart_item, &quit_item])?;
            let autostart_for_menu = autostart_item.clone();
            // 图标缺失（打包异常、精简过的资源）不该让托盘构建失败并把整个
            // 启动链带下去 —— 没有图标的托盘总比没有托盘强，日志里留一句就够了。
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("PrisTimer")
                .menu(&menu)
                .show_menu_on_left_click(false);
            match app.default_window_icon() {
                Some(icon) => tray = tray.icon(icon.clone()),
                None => log::log("托盘图标缺失，使用无图标托盘"),
            }
            tray
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "autostart" => {
                        let manager = app.autolaunch();
                        let enable = !manager.is_enabled().unwrap_or(false);
                        let result = if enable {
                            manager.enable()
                        } else {
                            manager.disable()
                        };
                        if result.is_ok() {
                            let _ = autostart_for_menu.set_checked(enable);
                        } else {
                            log::log("切换开机自启失败");
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 左键单击托盘图标 = 弹回主窗口（菜单留给右键）。
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            timer_start,
            timer_pause,
            timer_reset,
            timer_set_limit,
            pomodoro_set,
            pomodoro_current,
            pomodoro_config_get,
            pomodoro_config_set,
            recovered_session,
            timer_current,
            stats_daily,
            stats_summary,
            stats_tags,
            stats_tag_daily,
            stats_hourly,
            tag_set,
            tag_current,
            tag_goals_get,
            tag_goals_set,
            tag_rename,
            export_csv,
            export_report_csv,
            win_state_get,
            win_state_save,
            set_mini_shell,
            animate_window_to
        ])
}
