//! 科目标签命令：设置/当前值/目标/重命名。

use std::collections::BTreeMap;

use tauri::State;

use crate::state::SharedRecorder;

/// 设置下一条会话的标签（科目）。规范化规则：去首尾空白、限 20 字符、
/// 空串视为清除。规范化后的值落库为 `current_tag`，重启后沿用。
#[tauri::command]
pub(crate) fn tag_set(
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
pub(crate) fn tag_current(recorder: State<'_, SharedRecorder>) -> Option<String> {
    let guard = recorder.lock().ok()?;
    guard.current_tag().cloned()
}

/// 每日复习目标：科目 → 目标分钟数，落 `tag_goals` 键（JSON 对象）。
///
/// 校验规则与 `tag_set` 同源：科目去首尾空白、限 20 字；分钟数 1–1440
/// （一天上限）；目标总数最多 8 个。用 BTreeMap 是为了让序列化结果
/// 键序稳定，落库内容可读、可 diff。
#[tauri::command]
pub(crate) fn tag_goals_set(
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
pub(crate) fn tag_goals_get(
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
pub(crate) fn tag_rename(
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
