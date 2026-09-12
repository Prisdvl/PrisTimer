//! 会话的领域类型。
//!
//! 刻意不依赖 `rusqlite`：把「数据长什么样」与「怎么存取」分开，
//! 这样类型可以被上层自由引用，而不会把数据库驱动泄漏到业务代码里。

use std::fmt;

/// 会话类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum SessionKind {
    Stopwatch,
    Countdown,
}

impl SessionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopwatch => "stopwatch",
            Self::Countdown => "countdown",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "stopwatch" => Some(Self::Stopwatch),
            "countdown" => Some(Self::Countdown),
            _ => None,
        }
    }
}

impl fmt::Display for SessionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 会话的持久化状态。
///
/// 与 `pristimer-core::TimerState` 一一对应，但**刻意不共用同一个类型**：
/// 存储状态是数据库契约的一部分，一旦有历史数据落盘就不能随意改名；
/// 而引擎状态是内部实现细节，可以自由重构。把两者分开，重构引擎时就不会
/// 意外破坏旧数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum SessionState {
    Running,
    Paused,
    Finished,
}

impl SessionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Finished => "finished",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            "finished" => Some(Self::Finished),
            _ => None,
        }
    }
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一条完整的会话记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: i64,
    pub kind: SessionKind,
    pub tag: Option<String>,
    pub state: SessionState,
    /// 墙钟：会话第一次开始的那一刻。
    pub started_at: i64,
    /// 墙钟：当前运行段的起点。暂停时它等于暂停时刻。
    pub anchor_at: i64,
    /// 墙钟：会话结算时刻；未结算为 None。
    pub ended_at: Option<i64>,
    /// 上一次状态切换时已经累计的时长。
    pub elapsed_ms: i64,
    pub limit_ms: Option<i64>,
    /// 是否自然结束（倒计时到点）。手动停止为 false。
    pub completed: bool,
    pub note: Option<String>,
}

/// 启动时发现的一条未结算会话。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct Recovered {
    pub id: i64,
    pub kind: SessionKind,
    pub started_at: i64,
    /// 按锚点模型重算出的已用时长。
    pub elapsed_ms: u64,
    pub limit_ms: Option<u64>,
    /// 从会话开始到本次启动的墙钟跨度，用来判断是否值得接续。
    pub age_ms: u64,
}

/// 启动恢复的结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recovery {
    /// 可以接续的会话（最多一条，取最新的那条）。
    pub resume: Option<Recovered>,
    /// 被判定为已过期、已自动结算的会话数量。
    pub settled: usize,
}

/// 结算一条会话的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishOutcome {
    /// 已写入统计。
    Saved,
    /// 时长过短，已丢弃。
    Discarded,
}

/// 按天聚合的统计结果。日历与热力图直接消费这个结构。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct DailyStat {
    /// 本地时区的 `YYYY-MM-DD`。
    pub day: String,
    pub total_ms: i64,
    pub session_count: i64,
    pub completed_count: i64,
}

/// 按标签聚合的统计结果。无标签的会话归入空字符串键，由展示层起名。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct TagTotal {
    pub tag: String,
    pub total_ms: i64,
    pub session_count: i64,
}

/// 某一天、某一个标签的专注总时长。「目标达成天数」这类
/// 按「日 × 科目」统计的需求直接消费这个结构。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct TagDayTotal {
    /// 本地时区的 `YYYY-MM-DD`。
    pub day: String,
    /// 空串表示未标注。
    pub tag: String,
    pub total_ms: i64,
}

/// 某个本地小时（0–23）的专注总时长。「我什么时候效率最高」
/// 这类时段分析直接消费这个结构。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct HourTotal {
    /// 0–23，按会话**开始时刻**的本地小时归属。
    pub hour: u32,
    pub total_ms: i64,
    pub session_count: i64,
}
