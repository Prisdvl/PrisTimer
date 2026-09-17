//! 番茄钟编排层：把 core 的循环策略接到引擎命令与系统通知上。
//!
//! 分工 ——
//! - `pristimer-core::pomodoro`：纯策略（阶段怎么转），零依赖，可秒级单测；
//! - 这里：机制（收到 Finished 快照后怎么 Reset/SetLimit/Start、怎么发通知）；
//! - `Recorder`：只认 `set_recording` 开关，专注落库、休息跳过。
//!
//! 本结构体只保存状态与决策，**不做任何 IO** —— 真正的发命令 / 发通知
//! 在 `lib.rs` 的快照回调里完成，因为那里才拿得到 `AppHandle`。

use pristimer_core::{Phase, PomodoroConfig, PomodoroMachine};

/// 推给前端的番茄钟状态。
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PomodoroStatus {
    pub enabled: bool,
    pub phase: Phase,
    pub completed_focus: u32,
}

pub struct PomodoroManager {
    machine: PomodoroMachine,
    enabled: bool,
}

impl PomodoroManager {
    pub fn new() -> Self {
        Self {
            machine: PomodoroMachine::new(PomodoroConfig::default()),
            enabled: false,
        }
    }

    /// 用持久化恢复出的配置构造（启动时从 settings 表加载）。
    pub fn with_config(config: PomodoroConfig) -> Self {
        Self {
            machine: PomodoroMachine::new(config),
            enabled: false,
        }
    }

    pub fn config(&self) -> PomodoroConfig {
        self.machine.config()
    }

    /// 某个阶段的时长（按当前配置）。热应用配置时，编排层用它给
    /// 进行中的阶段重设引擎上限。
    pub fn duration_of(&self, phase: Phase) -> u64 {
        self.machine.duration_of(phase)
    }

    /// 热应用新配置。阶段与计数保持不变 —— 正在跑的阶段按引擎里已有的
    /// limit 走完，之后的阶段用新时长（是否重武装表盘由 lib.rs 决定）。
    pub fn apply_config(&mut self, config: PomodoroConfig) -> PomodoroStatus {
        self.machine.set_config(config);
        self.status()
    }

    pub fn status(&self) -> PomodoroStatus {
        PomodoroStatus {
            enabled: self.enabled,
            phase: self.machine.phase(),
            completed_focus: self.machine.completed_focus(),
        }
    }

    /// 开启番茄模式：循环从头计数，当前阶段回到专注。
    /// 注意这里**不自动开始计时** —— 让用户自己按下第一个「开始」。
    pub fn enable(&mut self) -> PomodoroStatus {
        self.machine.reset();
        self.enabled = true;
        self.status()
    }

    pub fn disable(&mut self) -> PomodoroStatus {
        self.enabled = false;
        self.status()
    }

    /// 当前阶段是否应该落库。关闭番茄模式时恒为 true（正常记录）。
    pub fn should_record(&self) -> bool {
        !self.enabled || self.machine.phase() == Phase::Focus
    }

    /// 番茄模式是否开启。
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 当前阶段（开启时即第一个专注）的时长。开启模式时用它武装引擎。
    pub fn armed_duration(&self) -> u64 {
        self.machine.duration()
    }

    /// 当前阶段到点后推进循环。返回 (新阶段, 新阶段时长)。
    pub fn advance(&mut self) -> (Phase, u64) {
        let phase = self.machine.advance();
        (phase, self.machine.duration_of(phase))
    }
}
