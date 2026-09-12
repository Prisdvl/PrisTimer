use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::clock::Clock;
use crate::timer::{TimerCore, TimerState};

pub const TICK_MS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct TimerSnapshot {
    pub state: TimerState,
    pub elapsed_ms: u64,
    pub remaining_ms: Option<u64>,
    pub limit_ms: Option<u64>,
}

/// 外部下发的指令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Start,
    Pause,
    Reset,
    SetLimit(Option<u64>),
    /// 从持久化记录恢复：把已用时长灌回引擎，并停在 `Paused`。
    Restore {
        elapsed_ms: u64,
        limit_ms: Option<u64>,
    },
    Shutdown,
}

/// 运行时句柄。持有命令发送端与工作线程。
pub struct TimerRuntime {
    cmd_tx: Sender<Command>,
    worker: Option<JoinHandle<()>>,
}

impl TimerRuntime {
    /// 启动后台计时线程。
    ///
    /// `on_update` 是唯一的外向接口：生产环境里它把快照 `emit` 给前端，
    /// 测试里它把快照塞进 channel。这就是依赖注入的价值。
    pub fn spawn<C, F>(clock: C, limit_ms: Option<u64>, on_update: F) -> Self
    where
        C: Clock,
        F: Fn(TimerSnapshot) + Send + 'static,
    {
        Self::spawn_with(clock, limit_ms, None, on_update)
    }

    /// 带「恢复状态」启动。
    ///
    /// 恢复必须在**进入循环之前**完成。否则线程的第一帧会是 `Idle / 0` 的快照，
    /// 持久化层会把它当成「用户按了重置」，刚恢复出来的会话记录就被结算掉了。
    /// 初始化顺序在这个项目里直接决定数据正确性，不能靠"反正很快"来赌。
    pub fn spawn_with<C, F>(
        clock: C,
        limit_ms: Option<u64>,
        restore: Option<(u64, Option<u64>)>,
        on_update: F,
    ) -> Self
    where
        C: Clock,
        F: Fn(TimerSnapshot) + Send + 'static,
    {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let worker = thread::spawn(move || run(clock, limit_ms, restore, cmd_rx, on_update));
        Self {
            cmd_tx,
            worker: Some(worker),
        }
    }

    /// 下发指令。线程已退出时静默忽略，不 panic。
    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// 克隆命令发送端。
    ///
    /// 存在的理由：番茄钟编排需要**在快照回调里反向下发指令**（收到 Finished
    /// 后自动 Reset → SetLimit → Start 下一阶段）。回调跑在工作线程上，
    /// 若经外层的 `Mutex<TimerRuntime>` 绕回来拿发送端，就会「线程等自己」。
    /// 克隆一个 `Sender` 直接入队，无锁、无死锁 —— `mpsc` 本来就是多生产者。
    pub fn sender(&self) -> Sender<Command> {
        self.cmd_tx.clone()
    }
}

impl Drop for TimerRuntime {
    /// 析构时先通知线程退出，再 `join` 等它真正结束。
    ///
    /// 少了这一步，进程退出时线程可能仍在跑；测试里则会泄漏线程。
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// 工作线程主体。
fn run<C, F>(
    clock: C,
    limit_ms: Option<u64>,
    restore: Option<(u64, Option<u64>)>,
    rx: Receiver<Command>,
    on_update: F,
) where
    C: Clock,
    F: Fn(TimerSnapshot),
{
    let mut core = TimerCore::new(clock, limit_ms);

    // 先恢复，再进循环 —— 这样第一帧快照就是 `Paused / 已用时长`。
    if let Some((elapsed_ms, limit_ms)) = restore {
        core.restore(elapsed_ms, limit_ms);
    }

    // 上一次已汇报的快照，用来抑制"什么都没变"的重复推送。
    let mut last: Option<TimerSnapshot> = None;

    loop {
        // recv_timeout 一物两用：有命令就立刻处理，没命令就等够一个节拍。
        match rx.recv_timeout(Duration::from_millis(TICK_MS)) {
            Ok(Command::Start) => core.start(),
            Ok(Command::Pause) => core.pause(),
            Ok(Command::Reset) => core.reset(),
            Ok(Command::SetLimit(limit)) => core.set_limit(limit),
            Ok(Command::Restore {
                elapsed_ms,
                limit_ms,
            }) => core.restore(elapsed_ms, limit_ms),
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }

        // 顺带判定倒计时是否到点。
        core.poll();

        let snapshot = TimerSnapshot {
            state: core.state(),
            elapsed_ms: core.elapsed_ms(),
            remaining_ms: core.remaining_ms(),
            limit_ms: core.limit_ms(),
        };

        // 运行中每拍都报（界面要跳秒）；静止时只在状态真的变化时才报。
        let running = snapshot.state == TimerState::Running;
        if running || last != Some(snapshot) {
            on_update(snapshot);
            last = Some(snapshot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use std::sync::Arc;
    use std::time::Instant;

    const EPOCH_MS: u64 = 1_700_000_000_000;
    const MIN_MS: u64 = 60_000;

    /// 等一个满足条件的快照，最多等 3 秒真机时间。
    fn wait_for<F>(rx: &Receiver<TimerSnapshot>, pred: F) -> TimerSnapshot
    where
        F: Fn(&TimerSnapshot) -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "等待符合条件的快照超时");
            match rx.recv_timeout(left) {
                Ok(snapshot) if pred(&snapshot) => return snapshot,
                Ok(_) => {}
                Err(e) => panic!("等待符合条件的快照失败: {e}"),
            }
        }
    }

    /// 造一套"测试侧能操纵时钟"的运行时。
    fn harness(limit_ms: Option<u64>) -> (Arc<FakeClock>, TimerRuntime, Receiver<TimerSnapshot>) {
        let clock = Arc::new(FakeClock::new(EPOCH_MS));
        let (tx, rx) = mpsc::channel();
        let runtime = TimerRuntime::spawn(clock.clone(), limit_ms, move |snapshot| {
            let _ = tx.send(snapshot);
        });
        (clock, runtime, rx)
    }

    #[test]
    fn stopwatch_runs_in_background() {
        let (clock, runtime, rx) = harness(None);

        runtime.send(Command::Start);
        // 先确认 Start 已被处理（锚点已建立），再推进假时钟 —— 否则存在竞态。
        wait_for(&rx, |s| s.state == TimerState::Running && s.elapsed_ms == 0);

        clock.advance(1_500);
        let snapshot = wait_for(&rx, |s| s.elapsed_ms >= 1_500);
        assert_eq!(snapshot.elapsed_ms, 1_500);
        assert_eq!(snapshot.remaining_ms, None);
    }

    #[test]
    fn paused_elapsed_is_frozen() {
        let (clock, runtime, rx) = harness(None);

        runtime.send(Command::Start);
        wait_for(&rx, |s| s.state == TimerState::Running && s.elapsed_ms == 0);
        clock.advance(2_000);
        wait_for(&rx, |s| s.elapsed_ms >= 2_000);

        runtime.send(Command::Pause);
        let paused = wait_for(&rx, |s| s.state == TimerState::Paused);
        assert_eq!(paused.elapsed_ms, 2_000);

        // 暂停期间假时钟继续走，但不应再有任何推送。
        clock.advance(10_000);
        assert!(
            rx.recv_timeout(Duration::from_millis(400)).is_err(),
            "暂停状态不应持续推送快照"
        );
    }

    #[test]
    fn countdown_finishes_with_capped_elapsed() {
        let (clock, runtime, rx) = harness(Some(1_000));

        runtime.send(Command::Start);
        wait_for(&rx, |s| s.state == TimerState::Running && s.elapsed_ms == 0);

        clock.advance(2_500);
        let finished = wait_for(&rx, |s| s.state == TimerState::Finished);
        assert_eq!(finished.elapsed_ms, 1_000);
        assert_eq!(finished.remaining_ms, Some(0));
    }

    #[test]
    fn set_limit_applies_while_idle() {
        let (_clock, runtime, rx) = harness(None);
        runtime.send(Command::SetLimit(Some(60_000)));
        let snapshot = wait_for(&rx, |s| s.limit_ms == Some(60_000));
        assert_eq!(snapshot.state, TimerState::Idle);
    }

    #[test]
    fn reset_returns_to_idle() {
        let (clock, runtime, rx) = harness(None);

        runtime.send(Command::Start);
        wait_for(&rx, |s| s.state == TimerState::Running && s.elapsed_ms == 0);
        clock.advance(3_000);
        wait_for(&rx, |s| s.elapsed_ms >= 3_000);

        runtime.send(Command::Reset);
        let snapshot = wait_for(&rx, |s| s.state == TimerState::Idle);
        assert_eq!(snapshot.elapsed_ms, 0);
    }

    #[test]
    fn restore_command_brings_back_a_paused_session() {
        let (clock, runtime, rx) = harness(None);

        runtime.send(Command::Restore {
            elapsed_ms: 5 * MIN_MS,
            limit_ms: None,
        });
        let restored = wait_for(&rx, |s| s.state == TimerState::Paused);
        assert_eq!(restored.elapsed_ms, 5 * MIN_MS);

        // 先确认 Start 已被处理（锚点已建立），再推进假时钟
        runtime.send(Command::Start);
        wait_for(&rx, |s| s.state == TimerState::Running && s.elapsed_ms == 5 * MIN_MS);

        clock.advance(2 * MIN_MS);
        let running = wait_for(&rx, |s| s.elapsed_ms >= 7 * MIN_MS);
        assert_eq!(running.elapsed_ms, 7 * MIN_MS);
    }

    #[test]
    fn spawn_with_restores_before_the_first_snapshot() {
        let clock = Arc::new(FakeClock::new(EPOCH_MS));
        let (tx, rx) = mpsc::channel();
        let runtime = TimerRuntime::spawn_with(
            clock,
            None,
            Some((5 * MIN_MS, None)),
            move |snapshot| {
                let _ = tx.send(snapshot);
            },
        );

        // 第一帧就必须是 Paused / 5 分钟。若顺序写反，这里会先收到 Idle / 0，
        // 持久化层就会把刚恢复的会话当成「用户重置」结算掉。
        let first = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(first.state, TimerState::Paused);
        assert_eq!(first.elapsed_ms, 5 * MIN_MS);

        drop(runtime);
    }

    #[test]
    fn drop_stops_worker_cleanly() {
        let (clock, runtime, rx) = harness(None);

        runtime.send(Command::Start);
        wait_for(&rx, |s| s.state == TimerState::Running);
        clock.advance(1_000);

        drop(runtime);
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_err());
    }
}
