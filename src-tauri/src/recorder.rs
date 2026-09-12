//! 会话记录器：把引擎的状态迁移翻译成数据库读写。
//!
//! 这一层承载的是**策略**，引擎提供的是**机制**。引擎只会说「我现在是 Running，
//! 已用 3 秒」；至于要不要落库、落成什么、什么时候结算 —— 全部由这里决定。
//! 正因为策略被隔离在这个文件里，`pristimer-core` 才能保持零数据库依赖。

use pristimer_core::{Clock, SystemClock, TimerSnapshot, TimerState};
use pristimer_store::{Recovered, SessionKind, Store};

/// 一条已经落库、尚未结算的会话。
struct OpenSession {
    id: i64,
    /// 上一拍看到的状态，用来区分「新会话」「从暂停继续」与「已经在跑」。
    last_state: TimerState,
    /// 上一拍看到的已用时长。
    ///
    /// 这个字段存在的唯一理由是「重置」：引擎重置之后上报的 `elapsed_ms` 是 0，
    /// 但用户真正专注的那段时长是重置**之前**的值。结算必须用上一拍的数值，
    /// 否则每次重置都会把整段专注时间丢掉。
    last_elapsed_ms: u64,
}

/// 下一步要执行的动作。先决策、再执行，避免在 `self.open` 的可变借用里
/// 再去借用 `self.store` —— 那是 Rust 里最容易把初学者卡住的地方。
enum Action {
    Begin,
    Refresh,
    Resume(i64),
}

pub struct Recorder {
    store: Store,
    open: Option<OpenSession>,
    /// 启动时恢复出来、等用户决定去留的会话。
    recovered: Option<Recovered>,
    /// 记录开关：番茄钟的休息阶段不产生专注统计，由编排层按阶段切换。
    /// 只约束**新会话的开启**；已开的会话照常走暂停 / 继续 / 结算。
    recording: bool,
    /// 下一条新会话携带的标签（科目）。已在跑的会话不受改动影响 ——
    /// 标签在落库那一刻就固定了，这是刻意设计：会话是历史事实。
    tag: Option<String>,
}

impl Recorder {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            open: None,
            recovered: None,
            recording: true,
            tag: None,
        }
    }

    /// 切换记录开关。休息开始前置 false，专注开始前置 true。
    pub fn set_recording(&mut self, enabled: bool) {
        self.recording = enabled;
    }

    /// 设置下一条新会话的标签。
    pub fn set_tag(&mut self, tag: Option<String>) {
        self.tag = tag;
    }

    /// 当前待用标签。
    pub fn current_tag(&self) -> Option<&String> {
        self.tag.as_ref()
    }

    /// 供统计查询复用同一个数据库连接。
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// 启动时调用：结算掉过期的残留会话，接管最新的一条等待用户决定。
    pub fn recover(&mut self) -> Option<Recovered> {
        let recovery = match self.store.recover_on_start(SystemClock.wall_ms()) {
            Ok(recovery) => recovery,
            Err(err) => {
                crate::log::log(format!("恢复未完成会话失败: {err}"));
                return None;
            }
        };

        if recovery.settled > 0 {
            crate::log::log(format!("启动时自动结算了 {} 条残留会话", recovery.settled));
        }

        let session = recovery.resume?;

        // 在这里就把记录接管过来。若不接管，用户一上来就按「重置」，
        // 这条记录会一直悬在库里（`ended_at` 为空），下次启动又被恢复一遍。
        self.open = Some(OpenSession {
            id: session.id,
            last_state: TimerState::Paused,
            last_elapsed_ms: session.elapsed_ms,
        });
        self.recovered = Some(session.clone());
        Some(session)
    }

    /// 启动时恢复出来的会话，供前端展示提示。
    pub fn recovery(&self) -> Option<&Recovered> {
        self.recovered.as_ref()
    }

    /// 每收到一帧引擎快照就调用一次。这里必须**永不 panic**：
    /// 它跑在计时线程上，一旦 panic 整个计时就停了。
    pub fn on_snapshot(&mut self, snapshot: TimerSnapshot) {
        match snapshot.state {
            TimerState::Running => self.on_running(snapshot),
            TimerState::Paused => self.on_paused(snapshot),
            TimerState::Finished => {
                // 倒计时跑到终点 —— 时长本身就带着「完成」语义。
                self.close(snapshot.elapsed_ms, true);
            }
            TimerState::Idle => {
                // 用户按了「重置」：引擎已清零，只能用上一拍记下的值。
                let elapsed = self.open.as_ref().map_or(0, |open| open.last_elapsed_ms);
                self.close(elapsed, false);
            }
        }
    }

    fn on_running(&mut self, snapshot: TimerSnapshot) {
        // 休息阶段：不新开会话，直接跳过。已开的会话不受影响
        // （记录开关只拦 Begin，不拦 Refresh / Resume / 结算）。
        if self.open.is_none() && !self.recording {
            return;
        }

        let action = match self.open.as_ref() {
            None => Action::Begin,
            Some(open) if open.last_state == TimerState::Running => Action::Refresh,
            Some(open) => Action::Resume(open.id),
        };

        let wall = SystemClock.wall_ms();

        match action {
            Action::Refresh => {
                if let Some(open) = self.open.as_mut() {
                    open.last_elapsed_ms = snapshot.elapsed_ms;
                }
            }
            Action::Resume(id) => {
                // 把「恢复时刻的已用时长」一并写回，否则下次崩溃时
                // 这段累积会静默消失。
                if let Err(err) = self.store.resume_session(id, wall, snapshot.elapsed_ms) {
                    crate::log::log(format!("继续会话失败: {err}"));
                }
                if let Some(open) = self.open.as_mut() {
                    open.last_state = TimerState::Running;
                    open.last_elapsed_ms = snapshot.elapsed_ms;
                }
            }
            Action::Begin => {
                let kind = if snapshot.limit_ms.is_some() {
                    SessionKind::Countdown
                } else {
                    SessionKind::Stopwatch
                };
                match self
                    .store
                    .begin_session(kind, self.tag.as_deref(), wall, snapshot.limit_ms)
                {
                    Ok(id) => {
                        self.open = Some(OpenSession {
                            id,
                            last_state: TimerState::Running,
                            last_elapsed_ms: snapshot.elapsed_ms,
                        });
                    }
                    Err(err) => crate::log::log(format!("新建会话失败: {err}")),
                }
            }
        }
    }

    fn on_paused(&mut self, snapshot: TimerSnapshot) {
        let (id, was_running) = match self.open.as_ref() {
            Some(open) => (open.id, open.last_state == TimerState::Running),
            None => return,
        };

        // 恢复出来的会话第一帧就是 Paused，此时无需写库 ——
        // `recover` 已经把「暂停 + 已用时长」落好了。
        if was_running {
            if let Err(err) = self
                .store
                .pause_session(id, SystemClock.wall_ms(), snapshot.elapsed_ms)
            {
                crate::log::log(format!("暂停会话失败: {err}"));
            }
        }

        if let Some(open) = self.open.as_mut() {
            open.last_state = TimerState::Paused;
            open.last_elapsed_ms = snapshot.elapsed_ms;
        }
    }

    /// 结算并放手当前会话。时长过短的会由存储层丢弃。
    fn close(&mut self, elapsed_ms: u64, completed: bool) {
        let Some(open) = self.open.take() else {
            return;
        };
        if let Err(err) = self.store.finish_session(
            open.id,
            SystemClock.wall_ms(),
            elapsed_ms,
            completed,
        ) {
            crate::log::log(format!("结算会话失败: {err}"));
        }
    }
}
