// ---------------------------------------------------------------------------
// 跨模块共享的运行时句柄与工具。
//
// P1-⑥ 自 lib.rs 拆出：命令模块（commands/*）与窗口模块（window）、
// lib.rs 的 setup 装配共用这些类型 —— 单一出处，避免循环依赖。
// ---------------------------------------------------------------------------

use std::sync::{Arc, Mutex};
use std::sync::mpsc::Sender;

use pristimer_core::{Command, TimerRuntime, TimerSnapshot};

use crate::pomodoro::PomodoroManager;
use crate::recorder::Recorder;

/// 取锁，**中毒也能继续用**。
///
/// 默认的 `.lock().unwrap()` 把「某个持锁线程曾经 panic 过」升级成「之后每一次
/// 访问都 panic」。对一个 release GUI 应用来说这没有意义：状态里躺着的是一份
/// 可能略有偏差的几何 / 计时数据，而代价却是整个窗口消失。
/// 取出内部数据继续用，把错误降级成"这一帧可能不准"，然后再靠日志去定位。
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 计时运行时的全局句柄。
pub(crate) struct AppTimer(pub(crate) Mutex<TimerRuntime>);

/// 记录器句柄：既要给计时线程的快照回调用，也要给统计命令用，
/// 因此用 `Arc` 让两边共享同一个实例（同一个数据库连接）。
pub(crate) type SharedRecorder = Arc<Mutex<Recorder>>;

/// 番茄钟编排器的全局句柄。快照回调与命令两边共享。
pub(crate) type SharedPomodoro = Arc<Mutex<PomodoroManager>>;

/// 最新一帧快照的缓存。
///
/// 事件是推送式的：前端 `listen` 订阅完成之前发出的快照不会补发，
/// 而计时线程在 `setup()` 阶段就发出了第一帧（恢复出来的暂停态）。
/// 所以前端挂载后需要主动拉一次当前值，这个缓存就是那次拉取的数据源。
pub(crate) type SnapshotCache = Arc<Mutex<Option<TimerSnapshot>>>;

/// 给快照回调反向下发命令用的发送端。
///
/// 回调在工作线程上跑，不能经 `AppTimer` 的互斥锁绕回来（线程等自己），
/// 所以另开一个 `Mutex<Option<Sender>>`，setup 完成后立刻填上。
/// 番茄模式在 setup 期间不可能已开启，因此「回调早于填充」的窗口是安全的。
pub(crate) type CommandCell = Arc<Mutex<Option<Sender<Command>>>>;
