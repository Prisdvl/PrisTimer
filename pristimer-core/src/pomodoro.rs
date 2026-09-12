//! 番茄钟循环状态机。
//!
//! 与计时引擎的关系：`TimerCore` 只认识「一次倒计时」；「专注完了接短休、
//! 短休完了接专注、连续 N 个专注后来一次长休」这件事是**循环策略**，
//! 引擎不该知道。所以它是纯逻辑状态机 —— 不碰时钟、不碰线程、不碰 Tauri，
//! `advance()` 只是「告诉我下一阶段是什么、该跑多久」。
//!
//! 编排（真的去 Reset/SetLimit/Start、发通知）在 `src-tauri` 的适配层完成，
//! 这里保持可以 `cargo test` 秒级验证的纯粹性。

/// 番茄钟的一个阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Phase {
    /// 专注（一个番茄）。
    Focus,
    /// 短休息。
    ShortBreak,
    /// 长休息（连续若干个专注后）。
    LongBreak,
}

impl Phase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::ShortBreak => "short_break",
            Self::LongBreak => "long_break",
        }
    }
}

/// 循环参数。单位统一毫秒，与引擎的 `limit_ms` 同一口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct PomodoroConfig {
    pub focus_ms: u64,
    pub short_break_ms: u64,
    pub long_break_ms: u64,
    /// 连续完成多少个专注后进入长休。
    pub focus_before_long: u32,
}

impl PomodoroConfig {
    /// 用户输入合法性校验：三个时长各限 1–120 分钟，轮数限 2–8。
    /// 返回 `Err(字段名)` 表示该字段越界。落库前调用，拒绝脏数据。
    pub fn validate(&self) -> Result<(), &'static str> {
        const MIN_MS: u64 = 60_000;
        const MAX_MS: u64 = 120 * 60_000;
        if self.focus_ms < MIN_MS || self.focus_ms > MAX_MS {
            return Err("focus_ms");
        }
        if self.short_break_ms < MIN_MS || self.short_break_ms > MAX_MS {
            return Err("short_break_ms");
        }
        if self.long_break_ms < MIN_MS || self.long_break_ms > MAX_MS {
            return Err("long_break_ms");
        }
        if !(2..=8).contains(&self.focus_before_long) {
            return Err("focus_before_long");
        }
        Ok(())
    }
}

impl Default for PomodoroConfig {
    /// 经典番茄工作法：25 / 5 / 15，每 4 个番茄一次长休。
    fn default() -> Self {
        const MIN: u64 = 60_000;
        Self {
            focus_ms: 25 * MIN,
            short_break_ms: 5 * MIN,
            long_break_ms: 15 * MIN,
            focus_before_long: 4,
        }
    }
}

/// 状态机本体。`completed_focus` 是跨阶段累积的「已完成番茄数」。
pub struct PomodoroMachine {
    config: PomodoroConfig,
    phase: Phase,
    completed_focus: u32,
}

impl PomodoroMachine {
    pub fn new(config: PomodoroConfig) -> Self {
        Self {
            config,
            phase: Phase::Focus,
            completed_focus: 0,
        }
    }

    pub fn config(&self) -> PomodoroConfig {
        self.config
    }

    /// 热更新循环参数。只改参数、不动阶段与计数。正在进行的阶段由引擎里
    /// 已下发的 `limit_ms` 决定实际时长，不受此次替换影响；`duration()`
    /// 由此刻起读新值，下一次 `advance()` 之后的阶段全部按新参数走。
    pub fn set_config(&mut self, config: PomodoroConfig) {
        self.config = config;
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn completed_focus(&self) -> u32 {
        self.completed_focus
    }

    /// 某个阶段的时长。
    pub fn duration_of(&self, phase: Phase) -> u64 {
        match phase {
            Phase::Focus => self.config.focus_ms,
            Phase::ShortBreak => self.config.short_break_ms,
            Phase::LongBreak => self.config.long_break_ms,
        }
    }

    /// 当前阶段的时长。
    pub fn duration(&self) -> u64 {
        self.duration_of(self.phase)
    }

    /// 当前阶段到点（Finished）后推进到下一阶段。
    ///
    /// 专注完成 → 计数 +1，按计数决定短休还是长休；休息完成 → 回到专注。
    /// 返回新阶段，调用方据此取时长、发通知。
    pub fn advance(&mut self) -> Phase {
        self.phase = match self.phase {
            Phase::Focus => {
                self.completed_focus += 1;
                if self.completed_focus.is_multiple_of(self.config.focus_before_long) {
                    Phase::LongBreak
                } else {
                    Phase::ShortBreak
                }
            }
            Phase::ShortBreak | Phase::LongBreak => Phase::Focus,
        };
        self.phase
    }

    /// 回到初始状态（用户关闭番茄模式或显式重开时调用）。
    pub fn reset(&mut self) {
        self.phase = Phase::Focus;
        self.completed_focus = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    /// 用「每 2 个专注进长休」的小配置缩短测试序列。
    fn machine() -> PomodoroMachine {
        PomodoroMachine::new(PomodoroConfig {
            focus_ms: 25 * MIN,
            short_break_ms: 5 * MIN,
            long_break_ms: 15 * MIN,
            focus_before_long: 2,
        })
    }

    #[test]
    fn default_config_is_the_classic_pomodoro() {
        let config = PomodoroConfig::default();
        assert_eq!(config.focus_ms, 25 * MIN);
        assert_eq!(config.short_break_ms, 5 * MIN);
        assert_eq!(config.long_break_ms, 15 * MIN);
        assert_eq!(config.focus_before_long, 4);
    }

    #[test]
    fn starts_in_focus() {
        let m = machine();
        assert_eq!(m.phase(), Phase::Focus);
        assert_eq!(m.completed_focus(), 0);
        assert_eq!(m.duration(), 25 * MIN);
    }

    #[test]
    fn cycle_goes_focus_short_focus_long() {
        let mut m = machine();
        // 第 1 个专注完成 → 短休
        assert_eq!(m.advance(), Phase::ShortBreak);
        assert_eq!(m.duration(), 5 * MIN);
        assert_eq!(m.completed_focus(), 1);
        // 短休完成 → 回到专注
        assert_eq!(m.advance(), Phase::Focus);
        // 第 2 个专注完成 → 达到 focus_before_long，进长休
        assert_eq!(m.advance(), Phase::LongBreak);
        assert_eq!(m.duration(), 15 * MIN);
        assert_eq!(m.completed_focus(), 2);
        // 长休完成 → 新一轮专注
        assert_eq!(m.advance(), Phase::Focus);
        // 第 3 个专注 → 计数 3 % 2 == 1，又是短休
        assert_eq!(m.advance(), Phase::ShortBreak);
        assert_eq!(m.completed_focus(), 3);
    }

    #[test]
    fn full_cycle_repeats_indefinitely() {
        let mut m = machine();
        // 跑满 3 轮（每轮 = 专注×2 + 长休前的短休），阶段序列必须严格循环。
        for round in 0..3 {
            assert_eq!(m.advance(), Phase::ShortBreak, "round {round}");
            assert_eq!(m.advance(), Phase::Focus);
            assert_eq!(m.advance(), Phase::LongBreak, "round {round}");
            assert_eq!(m.advance(), Phase::Focus);
        }
        assert_eq!(m.completed_focus(), 6);
    }

    #[test]
    fn reset_returns_to_a_fresh_cycle() {
        let mut m = machine();
        m.advance();
        m.advance();
        m.reset();
        assert_eq!(m.phase(), Phase::Focus);
        assert_eq!(m.completed_focus(), 0);
        assert_eq!(m.duration(), 25 * MIN);
    }

    #[test]
    fn validate_accepts_the_default_and_rejects_outliers() {
        assert_eq!(PomodoroConfig::default().validate(), Ok(()));

        let too_short = PomodoroConfig {
            focus_ms: 30_000, // 半分钟，低于下限 1 分钟
            ..PomodoroConfig::default()
        };
        assert_eq!(too_short.validate(), Err("focus_ms"));

        let bad_rounds = PomodoroConfig {
            focus_before_long: 9,
            ..PomodoroConfig::default()
        };
        assert_eq!(bad_rounds.validate(), Err("focus_before_long"));
    }

    #[test]
    fn set_config_keeps_phase_and_applies_on_next_advance() {
        let mut m = machine();
        m.advance(); // 专注 #1 完成，正在短休
        assert_eq!(m.phase(), Phase::ShortBreak);

        let custom = PomodoroConfig {
            focus_ms: 40 * MIN,
            short_break_ms: 8 * MIN,
            long_break_ms: 20 * MIN,
            focus_before_long: 2,
        };
        m.set_config(custom);

        // 阶段与计数不动：短休还是短休，已完成番茄还是 1
        assert_eq!(m.phase(), Phase::ShortBreak);
        assert_eq!(m.completed_focus(), 1);
        // duration() 由此刻起读新配置
        assert_eq!(m.duration(), 8 * MIN);

        // 短休结束进入专注，按新参数走
        m.advance();
        assert_eq!(m.phase(), Phase::Focus);
        assert_eq!(m.duration(), 40 * MIN);
    }
}
