//! pristimer-core —— 与框架无关的计时引擎。
//!
//! 本 crate 不依赖 Tauri、不依赖任何 GUI 框架，因此可以用 `cargo test` 秒级验证。
//!
//! 【关键规则】`src/` 下的 `.rs` 文件不会自动成为模块。
//! 每个模块都必须在 crate 根（也就是这个文件）里用 `mod xxx;` 显式声明，
//! 声明之后编译器才会去磁盘上找 `src/xxx.rs`。

pub mod clock;
pub mod pomodoro;
pub mod runtime;
pub mod timer;

pub use clock::{Clock, FakeClock, SystemClock};
pub use pomodoro::{Phase, PomodoroConfig, PomodoroMachine};
pub use runtime::{Command, TimerRuntime, TimerSnapshot};
pub use timer::{TimerCore, TimerState};
