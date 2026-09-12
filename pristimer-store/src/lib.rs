//! pristimer-store —— 计时器的 SQLite 持久化层。
//!
//! 本 crate 不认识 Tauri、不认识计时引擎，只做三件事：
//! 建表、读写会话、聚合统计。因为边界干净，它可以用内存数据库完整测试。

pub mod session;
pub mod store;

pub use session::{
    DailyStat, FinishOutcome, HourTotal, Recovered, Recovery, SessionKind, SessionRow, SessionState,
    TagDayTotal, TagTotal,
};
pub use store::{MAX_RESUME_GAP_MS, MIN_SESSION_MS, Result, Store, StoreError};
