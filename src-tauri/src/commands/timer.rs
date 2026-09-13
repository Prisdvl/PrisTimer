//! 计时命令：启停/重置/限长，以及当前快照与启动恢复的拉取。

use tauri::State;

use pristimer_core::{Command, TimerSnapshot};
use pristimer_store::Recovered;

use crate::state::{lock, AppTimer, SharedRecorder, SnapshotCache};

#[tauri::command]
pub(crate) fn timer_start(state: State<'_, AppTimer>) {
    lock(&state.0).send(Command::Start);
}

#[tauri::command]
pub(crate) fn timer_pause(state: State<'_, AppTimer>) {
    lock(&state.0).send(Command::Pause);
}

#[tauri::command]
pub(crate) fn timer_reset(state: State<'_, AppTimer>) {
    lock(&state.0).send(Command::Reset);
}

#[tauri::command]
pub(crate) fn timer_set_limit(limit_ms: Option<u64>, state: State<'_, AppTimer>) {
    lock(&state.0).send(Command::SetLimit(limit_ms));
}
/// 启动时发现的可接续会话。前端用它决定是否显示提示条。
#[tauri::command]
pub(crate) fn recovered_session(recorder: State<'_, SharedRecorder>) -> Option<Recovered> {
    let guard = recorder.lock().ok()?;
    guard.recovery().cloned()
}

/// 当前计时快照。前端挂载时主动拉取一次，弥补订阅前丢失的推送。
#[tauri::command]
pub(crate) fn timer_current(cache: State<'_, SnapshotCache>) -> Option<TimerSnapshot> {
    *cache.lock().ok()?
}
