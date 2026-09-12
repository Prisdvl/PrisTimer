use crate::clock::Clock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum TimerState {
    Idle,     //空闲/重置
    Running,  //计时中
    Paused,   //暂停
    Finished, //结束
}

pub struct TimerCore<C: Clock> {
    clock: C,              //时钟
    state: TimerState,     //状态
    wall_anchor_ms: u64,   //墙钟锚点
    mono_anchor_ms: u64,   //单调时钟锚点
    accumulated_ms: u64,   //保存的时间
    limit_ms: Option<u64>, //上限时间：Some=倒计时；None=秒表
}

impl<C: Clock> TimerCore<C> {
    // 创建实例
    pub fn new(clock: C, limit_ms: Option<u64>) -> Self {
        Self {
            clock,
            state: TimerState::Idle,
            wall_anchor_ms: 0,
            mono_anchor_ms: 0,
            accumulated_ms: 0,
            limit_ms,
        }
    }
    // getter
    pub fn clock(&self) -> &C {
        &self.clock
    }

    pub fn state(&self) -> TimerState {
        self.state
    }

    pub fn wall_anchor_ms(&self) -> u64 {
        self.wall_anchor_ms
    }

    pub fn limit_ms(&self) -> Option<u64> {
        self.limit_ms
    }

    pub fn set_limit(&mut self, limit_ms: Option<u64>) {
        if self.state == TimerState::Idle {
            self.limit_ms = limit_ms;
        }
    }

    /// 从持久化记录恢复一段已用时长，并停在 `Paused`。
    ///
    /// 为什么恢复成暂停态而不是直接开跑：应用关闭的这段时间里，我们无法确认
    /// 用户是否真的在专注。停在暂停态、让用户自己决定「继续」还是「重置」，
    /// 是唯一不会篡改统计的选择。
    ///
    /// 与 `set_limit` 不同，这里不受 `Idle` 限制 —— 恢复本来就是为了把引擎
    /// 从空白状态扶到有内容的状态。
    pub fn restore(&mut self, elapsed_ms: u64, limit_ms: Option<u64>) {
        self.state = TimerState::Paused;
        self.accumulated_ms = elapsed_ms;
        self.limit_ms = limit_ms;
        // 锚点清零：`Paused` 态不参与 `elapsed_ms` 的计算，
        // 等用户按下「继续」时 `start()` 会重新建立锚点。
        self.wall_anchor_ms = 0;
        self.mono_anchor_ms = 0;
    }

    // 重置
    pub fn reset(&mut self) {
        self.state = TimerState::Idle;
        self.accumulated_ms = 0;
        self.wall_anchor_ms = 0;
        self.mono_anchor_ms = 0;
    }
    // 启动
    pub fn start(&mut self) {
        match self.state {
            TimerState::Running => return,
            TimerState::Finished => self.reset(),
            _ => {}
        }
        self.wall_anchor_ms = self.clock.wall_ms();
        self.mono_anchor_ms = self.clock.monotonic_ms();
        self.state = TimerState::Running;
    }
    // 暂停
    pub fn pause(&mut self) {
        if self.state != TimerState::Running {
            return;
        }
        self.accumulated_ms += self.segment_ms();
        self.state = TimerState::Paused;
    }
    // 片段流逝时间
    fn segment_ms(&self) -> u64 {
        self.clock
            .monotonic_ms()
            .saturating_sub(self.mono_anchor_ms)
    }
    // 总消耗时间
    pub fn elapsed_ms(&self) -> u64 {
        match self.state {
            TimerState::Running => self.accumulated_ms + self.segment_ms(),
            _ => self.accumulated_ms,
        }
    }
    // 剩余时间
    pub fn remaining_ms(&self) -> Option<u64> {
        let limit = self.limit_ms?;
        Some(limit.saturating_sub(self.elapsed_ms()))
    }
    // 外部按频率调用
    pub fn poll(&mut self) -> bool {
        if self.state != TimerState::Running {
            return false;
        }
        let Some(limit) = self.limit_ms else {
            return false;
        };
        if self.elapsed_ms() >= limit {
            self.accumulated_ms = limit;
            self.state = TimerState::Finished;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;

    const EPOCH_MS: u64 = 1_700_000_000_000;
    const SEC: u64 = 1_000;
    const MIN: u64 = 60 * SEC;
    fn new_stopwatch() -> TimerCore<FakeClock> {
        TimerCore::new(FakeClock::new(EPOCH_MS), None)
    }

    fn new_countdown(limit_ms: u64) -> TimerCore<FakeClock> {
        TimerCore::new(FakeClock::new(EPOCH_MS), Some(limit_ms))
    }

    #[test]
    fn stopwatch_tracks_elapsed() {
        let mut t = new_stopwatch();
        t.start();
        t.clock().advance(1_500);
        assert_eq!(t.elapsed_ms(), 1_500);
    }

    #[test]
    fn paused_time_is_not_counted() {
        let mut t = new_stopwatch();
        t.start();
        t.clock().advance(1_500);
        t.pause();

        t.clock().advance(10 * SEC); // 暂停期间时间在走，但不应计入
        assert_eq!(t.elapsed_ms(), 1_500);
        assert_eq!(t.state(), TimerState::Paused);

        t.start(); // 继续
        t.clock().advance(500);
        assert_eq!(t.elapsed_ms(), 2_000);
    }

    #[test]
    fn reset_clears_everything() {
        let mut t = new_stopwatch();
        t.start();
        t.clock().advance(5 * SEC);
        t.reset();
        assert_eq!(t.elapsed_ms(), 0);
        assert_eq!(t.state(), TimerState::Idle);
        assert_eq!(t.wall_anchor_ms(), 0);
    }

    #[test]
    fn countdown_completes_exactly_at_limit() {
        let mut t = new_countdown(25 * MIN);
        t.start();

        t.clock().advance(24 * MIN);
        assert!(!t.poll());
        assert_eq!(t.remaining_ms(), Some(MIN));

        t.clock().advance(90 * SEC); // 一次跨越终点
        assert!(t.poll());
        assert_eq!(t.elapsed_ms(), 25 * MIN);
        assert_eq!(t.remaining_ms(), Some(0));
        assert_eq!(t.state(), TimerState::Finished);
    }

    #[test]
    fn poll_is_idempotent_after_finish() {
        let mut t = new_countdown(MIN);
        t.start();
        t.clock().advance(2 * MIN);
        assert!(t.poll());
        assert!(!t.poll());
    }

    #[test]
    fn an_hour_of_ticks_produces_no_drift() {
        let mut t = new_stopwatch();
        t.start();
        for _ in 0..3_600 {
            t.clock().advance(SEC);
            t.poll();
        }
        assert_eq!(t.elapsed_ms(), 3_600 * SEC);
    }

    #[test]
    fn wall_anchor_records_wall_clock() {
        let mut t = new_stopwatch();
        t.clock().advance(7 * SEC);
        t.start();
        assert_eq!(t.wall_anchor_ms(), EPOCH_MS + 7 * SEC);
    }

    #[test]
    fn limit_change() {
        let mut t = new_stopwatch();
        // 场景1：Idle状态，可以修改limit
        t.set_limit(Some(1000));
        assert_eq!(t.limit_ms(), Some(1000));
        // 场景2：Running状态，修改无效
        t.start();
        t.set_limit(Some(9999));
        assert_eq!(t.limit_ms(), Some(1000)); // 仍然是旧值，没改成9999

        // 场景3：Paused状态，修改无效
        t.pause();
        t.set_limit(Some(8888));
        assert_eq!(t.limit_ms(), Some(1000));

        // 场景4：Finished状态，修改无效
        // 先把计时器跑超时进入Finished
        t.start();
        t.clock().advance(2000);
        t.poll(); // 触发超时，state变为Finished
        assert_eq!(t.state(), TimerState::Finished);
        t.set_limit(Some(7777));
        assert_eq!(t.limit_ms(), Some(1000));
    }

    #[test]
    fn restore_brings_back_a_paused_session() {
        let mut t = new_stopwatch();
        t.restore(5 * MIN, None);

        assert_eq!(t.state(), TimerState::Paused);
        assert_eq!(t.elapsed_ms(), 5 * MIN);

        // 恢复后是暂停态：时间流逝不应计入
        t.clock().advance(3 * MIN);
        assert_eq!(t.elapsed_ms(), 5 * MIN);

        // 用户按下「继续」，从 5 分钟处接着跑
        t.start();
        t.clock().advance(2 * MIN);
        assert_eq!(t.elapsed_ms(), 7 * MIN);
    }
}
