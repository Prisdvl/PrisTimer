//! 统计命令：日聚合 / 总览 / 按标签 / 标签日聚合 / 小时分布。

use tauri::State;

use pristimer_store::{DailyStat, HourTotal, TagDayTotal, TagTotal};

use crate::state::SharedRecorder;

/// 统计总览。
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatsSummary {
    total_ms: i64,
    session_count: i64,
}

/// 区间统计：某个日期范围内每天的聚合数据，日历与热力图直接消费。
/// 日期格式 `YYYY-MM-DD`（本地时区），区间含两端。
#[tauri::command]
pub(crate) fn stats_daily(
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
pub(crate) fn stats_summary(recorder: State<'_, SharedRecorder>) -> Result<StatsSummary, String> {
    let guard = recorder.lock().map_err(|err| err.to_string())?;
    let store = guard.store();
    Ok(StatsSummary {
        total_ms: store.total_ms().map_err(|err| err.to_string())?,
        session_count: store.finished_count().map_err(|err| err.to_string())?,
    })
}

/// 文件名白名单：只放行 ASCII 字母数字与 `_-.`，杜绝路径穿越与非法字符。
/// 按标签聚合的专注时长（科目统计）。日期范围含两端，与 stats_daily 同口径。
#[tauri::command]
pub(crate) fn stats_tags(
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
pub(crate) fn stats_tag_daily(
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
pub(crate) fn stats_hourly(
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

