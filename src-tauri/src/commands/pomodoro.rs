//! 番茄钟命令：开关、当前状态、配置读写。

use tauri::State;

use pristimer_core::{Command, Phase, PomodoroConfig, TimerState};

use crate::pomodoro::PomodoroStatus;

use crate::state::{lock, AppTimer, SharedPomodoro, SharedRecorder, SnapshotCache};

/// 开关番茄模式。返回切换后的状态，前端也可走事件通道。
#[tauri::command]
pub(crate) fn pomodoro_set(
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
pub(crate) fn pomodoro_current(pomodoro: State<'_, SharedPomodoro>) -> PomodoroStatus {
    lock(&pomodoro).status()
}

/// 当前番茄钟配置（前端设置面板的初始值）。
#[tauri::command]
pub(crate) fn pomodoro_config_get(pomodoro: State<'_, SharedPomodoro>) -> PomodoroConfig {
    lock(&pomodoro).config()
}

/// 更新番茄钟配置：校验 → 落库 → 热应用。
/// 若模式开启且引擎正停在 Idle（表盘未在计时），立即按新专注时长重武装。
#[tauri::command]
pub(crate) fn pomodoro_config_set(
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

/// 番茄阶段到点后给用户的通知文案。
pub(crate) fn pomodoro_notice(phase: Phase, duration_ms: u64) -> (String, String) {
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
