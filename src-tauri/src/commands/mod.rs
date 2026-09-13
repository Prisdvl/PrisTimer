//! Tauri 命令模块：按域拆分（timer / pomodoro / stats / export / tags）。
//!
//! P1-⑥ 自 lib.rs 拆出，命令函数体逐字搬迁；`pub use` 供 lib.rs 的
//! `generate_handler!` 以平铺路径引用。

pub mod export;
pub mod pomodoro;
pub mod stats;
pub mod tags;
pub mod timer;

