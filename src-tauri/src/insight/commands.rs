//! 洞察子系统的 Tauri 命令与后台刷新任务。
//!
//! 命令清单（全部 `#[tauri::command]` 异步函数）：
//!
//! | 命令 | 用途 | 会触发真实扫描吗 |
//! |---|---|---|
//! | `insight_current`      | 读当前快照 | 否，纯读缓存 |
//! | `bt_devices`           | 查蓝牙设备 | 是（受 10s 节流闸门约束） |
//! | `bt_refresh`           | 用户「立即刷新」蓝牙 | 是（同上，并回报是否被节流） |
//! | `quota_query`          | 查 opencode-go 额度 | 是（真发 HTTP） |
//! | `quota_config_get/set` | 读写 API Key 与接口地址 | 否 |
//! | `overlay_toggle`       | 开关托盘悬浮信息窗 | 否 |
//!
//! ★ 「异步函数不能阻塞 UI」这句话在本模块的落法：
//!
//!   Tauri 的 `#[tauri::command] async fn` 跑在 tokio 运行时上，
//!   **不是**主线程，所以 async 本身已经保证了不卡 UI 线程。
//!   真正要防的是另一件事：在 async 函数里直接调阻塞 API
//!   （WinHTTP 同步请求、WinRT 的 `.get()`），会把 tokio 的
//!   工作线程占住；工作线程数等于 CPU 核数，占满几个之后
//!   **整个应用所有命令都会排队**，表现就是"界面卡住"。
//!   所以模块里所有阻塞调用都包在 `spawn_blocking` 里 ——
//!   见 `bluetooth` / `quota` 两个子模块。

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::insight::device;
use crate::insight::quota::{self};
use crate::insight::{BtStatus, InsightSnapshot, InsightState, QuotaStatus};

/// 前端订阅的快照事件名。
///
/// 与项目里 `timer:update` / `pomodoro:update` 同一命名风格
/// （`域:动作`）。前端 `listen<InsightSnapshot>("insight:update", ...)`。
pub const EVENT_UPDATE: &str = "insight:update";

// ---------------------------------------------------------------------------
// 读取类命令
// ---------------------------------------------------------------------------

/// 读当前快照。**纯读，绝不触发扫描。**
///
/// 前端每次挂载都调它，所以它必须便宜。所有真实扫描都由后台任务
/// 或用户显式点刷新触发，结果写进这里读的缓存。
#[tauri::command]
pub async fn insight_current(state: State<'_, std::sync::Arc<InsightState>>) -> Result<InsightSnapshot, String> {
    Ok(state.snapshot())
}

/// 查询蓝牙设备列表。
///
/// 返回的是**结构化数据**而不是拼好的字符串：设备名、类型、电量各自独立，
/// 前端想怎么排版就怎么排版（状态栏一行串起来、托盘菜单一条一个设备、
/// 将来加个悬浮卡片画进度条，都不需要后端改）。
///
/// 受 10 秒节流闸门约束（见 `device::DeviceCache`）。被节流时返回的是
/// 上一次的扫描结果 —— 对调用方来说**是无感的**：拿到的依然是一份
/// 正确（只是可能旧了 30 秒以内）的数据。这样前端不需要写
/// "被节流了怎么办"的分支。
#[tauri::command]
pub async fn bt_devices(
    app: AppHandle,
    state: State<'_, std::sync::Arc<InsightState>>,
) -> Result<BtStatus, String> {
    let state = state.inner().clone();
    // 再克隆一份进闭包 —— `state` 本身在 await 之后还要用（写缓存 + 推送），
    // 不能 move 进去。`Arc` 的克隆是引用计数自增，成本可忽略。
    let inner = state.clone();
    // ★ 整个「判闸门 + 扫描 + 回填缓存」都在 spawn_blocking 里跑。
    //
    //   注意这里**没有**先 await 再进 spawn_blocking：WinRT 的调用链
    //   全程同步阻塞，放进 async 块里没有任何收益，反而让
    //   「到底哪一段在阻塞」变得难以追踪。
    let status = tauri::async_runtime::spawn_blocking(move || inner.devices.refresh())
        .await
        .map_err(|err| format!("蓝牙查询任务异常：{err}"))?;

    state.set_bluetooth(status.clone());
    publish(&app, &state);
    Ok(status)
}

/// 用户显式触发的蓝牙刷新。
///
/// 与 `bt_devices` 的唯一区别是**会告诉调用方是否被节流**。
/// 用户连点刷新按钮时，前端可以据此显示「刚刚才扫过，稍候再试」，
/// 而不是让按钮看起来没反应。
#[tauri::command]
pub async fn bt_refresh(
    app: AppHandle,
    state: State<'_, std::sync::Arc<InsightState>>,
) -> Result<RefreshOutcome, String> {
    let state = state.inner().clone();
    // 再克隆一份进去处理阻塞任务 —— `state` 本身在 await 之后还要用
    // （写缓存 + 推送），不能把它 move 进闭包。
    let inner = state.clone();
    let (status, gate) = tauri::async_runtime::spawn_blocking(move || inner.devices.force())
        .await
        .map_err(|err| format!("蓝牙刷新任务异常：{err}"))?;

    state.set_bluetooth(status.clone());
    publish(&app, &state);

    Ok(RefreshOutcome {
        throttled: gate == device::Gate::Throttled,
        bluetooth: status,
    })
}

/// `bt_refresh` 的返回体。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshOutcome {
    /// 这次请求是否被节流（true = 返回的是缓存，没有真的扫描）。
    pub throttled: bool,
    /// 蓝牙状态。
    pub bluetooth: BtStatus,
}

// ---------------------------------------------------------------------------
// 额度
// ---------------------------------------------------------------------------

/// 查询 opencode-go 订阅剩余额度。
///
/// 网络请求 + 响应解析全程在 `spawn_blocking` 里 —— WinHTTP 是同步 API，
/// 直接在 async 上下文里调会把 tokio 工作线程挂住最多 8 秒（超时值），
/// 期间其它所有命令都在排队。
#[tauri::command]
pub async fn quota_query(
    app: AppHandle,
    state: State<'_, std::sync::Arc<InsightState>>,
) -> Result<QuotaStatus, String> {
    let state = state.inner().clone();
    // 配置在进阻塞线程**之前**取出来：`quota_config()` 会短暂持锁，
    // 持锁进去再发网络请求就成了「持锁跨阻塞调用」，后台刷新任务
    // 要是同时想改配置就会被堵住好几秒。
    let config = state.quota_config();

    let status = tauri::async_runtime::spawn_blocking(move || quota::fetch_blocking(&config))
        .await
        .map_err(|err| format!("额度查询任务异常：{err}"))?;

    state.set_quota(status.clone());
    publish(&app, &state);
    Ok(status)
}

/// 读额度配置。**API Key 会被打码。**
///
/// ★ 为什么要打码：配置面板要回显"当前填的是什么"，
///   但把一个完整 Key 送进 WebView 意味着它可能出现在
///   DevTools、崩溃转储、或者用户截的图里。回显只需要
///   "有配置 / 是哪个 Key"，不需要原文。
#[tauri::command]
pub async fn quota_config_get(
    state: State<'_, std::sync::Arc<InsightState>>,
) -> Result<QuotaConfigView, String> {
    let config = state.quota_config();
    let endpoint = config.normalized_endpoint();
    // ★ 判定"是否还在用内置端点"要按**规范化之后**比：
    //   用户存了 `…/zen/go/v1`（少 /usage）时，归一后就是内置端点，
    //   此时提示"当前使用内置端点"才与事实相符 —— 按原始字符串
    //   判定会把它误标成"自建网关"，把用户引向去猜路径。
    let endpoint_is_default = endpoint == quota::DEFAULT_ENDPOINT;
    Ok(QuotaConfigView {
        has_key: !config.api_key.trim().is_empty(),
        key_hint: mask_key(&config.api_key),
        // ★ 回显**实际会请求的地址**（而不是原始输入）：用户没填时这里
        //   会显示内置的 opencode Go 用量端点，一眼就知道东西发去了哪儿。
        //   这正是修掉"填了网关根路径 → 404 却不知道为什么"的关键一步。
        endpoint,
        endpoint_is_default,
    })
}

/// 配置面板看到的额度设置。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaConfigView {
    /// 是否已经配了 Key。前端据此决定「未配置」提示显不显示。
    pub has_key: bool,
    /// 打码后的 Key（`sk-abc…9f2`），只用于让用户确认"填的是这一个"。
    pub key_hint: String,
    /// 实际使用的接口地址（留空时为内置默认端点）。
    pub endpoint: String,
    /// 该地址是否来自内置默认值（用户没自定义）。
    pub endpoint_is_default: bool,
}

/// 写入额度配置，并立刻触发一次查询。
///
/// 立刻查是刻意的：用户刚填完 Key，界面上应该马上出现结果，
/// 而不是等下一个 5 分钟周期。同时这也是一次即时校验 ——
/// Key 填错了，用户在这一刻就能看到「API Key 无效」。
#[tauri::command]
pub async fn quota_config_set(
    app: AppHandle,
    state: State<'_, std::sync::Arc<InsightState>>,
    api_key: Option<String>,
    endpoint: Option<String>,
) -> Result<QuotaStatus, String> {
    let state = state.inner().clone();
    let mut current = state.quota_config();

    // 只更新传进来的字段（合并语义）。
    //
    // ★ 这里**必须**区分「没传」和「传了空串」：
    //   · `None`        → 前端没动这个字段，保留原值；
    //   · `Some("")`    → 前端清空了它，要真的清掉；
    //   · `Some("  ")`  → trim 之后是空，同样视为清空。
    //   用 `unwrap_or(current)` 一把梭会把"清空"变成"没改"，
    //   用户点了清除按钮却发现 Key 还在 —— 这类 bug 极难被发现。
    if let Some(key) = api_key {
        current.api_key = key.trim().to_string();
    }
    if let Some(url) = endpoint {
        let trimmed = url.trim().to_string();
        // 地址非空时必须能被解析，否则把一个必定失败的地址存下来，
        // 用户之后每次刷新都看到"网络不可用"，却不知道是自己填错了。
        // （parse_url 内部会自动补 https://，所以"api.xxx.com/…"
        //   这种用户常填的形态也能通过。）
        if !trimmed.is_empty() {
            crate::insight::http::parse_url(&trimmed).map_err(|err| err.message())?;
        }
        current.endpoint = trimmed;
    }

    state.set_quota_config(current.clone());

    // ★ 写一条配置变更日志 —— "没反应"时第一件事就是看它。
    //   记的是**规范化之后的请求地址**：用户填了网关根路径时，
    //   这里能直接看出我们实际打的是哪个 URL。
    crate::log::log(format!(
        "额度配置已更新：Key={} 请求地址={}",
        if current.api_key.is_empty() {
            "(未设置)"
        } else {
            "(已设置)"
        },
        current.normalized_endpoint(),
    ));

    // ★ 落库持久化。
    //
    //   不加这一步，用户填完 Key 重启应用就"又没配置了" —— 而界面上
    //   只会回到「未配置 API Key」，用户完全不知道发生了什么。
    //   与项目里 `pomodoro_config` / `current_tag` 用同一张 settings 表
    //   与同一套 JSON 序列化方式。
    //
    //   失败**不阻断**本次保存：内存里已经生效了，本次会话能用；
    //   写库失败只记日志（磁盘满、库被锁等）。
    if let Some(recorder) = app.try_state::<crate::state::SharedRecorder>() {
        if let Ok(guard) = recorder.lock() {
            match serde_json::to_string(&current) {
                Ok(json) => {
                    if let Err(err) = guard.store().set_setting(quota::SETTINGS_KEY, &json) {
                        crate::log::log(format!("额度配置落库失败（本次会话仍生效）: {err}"));
                    }
                }
                Err(err) => crate::log::log(format!("额度配置序列化失败: {err}")),
            }
        }
    }

    if !current.is_ready() {
        state.set_quota(QuotaStatus::NotConfigured);
        publish(&app, &state);
        return Ok(QuotaStatus::NotConfigured);
    }

    let status = tauri::async_runtime::spawn_blocking(move || quota::fetch_blocking(&current))
        .await
        .map_err(|err| format!("额度查询任务异常：{err}"))?;
    state.set_quota(status.clone());
    publish(&app, &state);
    Ok(status)
}

/// Key 打码：保留开头 6 位与结尾 3 位，中间省略。
///
/// 短 Key（少于 12 位）整体打码 —— 保留头尾之后剩不下什么，
/// 反而会把整个 Key 暴露出去（比如 8 位的 Key，留 6+3 就等于全露了）。
fn mask_key(key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 12 {
        return "•".repeat(chars.len());
    }
    let head: String = chars.iter().take(6).collect();
    let tail: String = chars.iter().skip(chars.len() - 3).collect();
    format!("{head}…{tail}")
}

// ---------------------------------------------------------------------------
// 托盘悬浮信息窗
// ---------------------------------------------------------------------------

/// 开关托盘旁的悬浮信息窗（托盘 tooltip 之外的「常驻展示」通道）。
///
/// ★ 为什么托盘 tooltip 之外还要一个悬浮窗：
///
///   Windows 的托盘 tooltip 只能**鼠标悬停时**出现、约 5 秒后自动消失，
///   而且系统不允许程序主动控制它。需求是「程序最小化时仍需要展示这两组
///   信息」—— 悬停才能看、5 秒就没，显然不满足"展示"。
///   所以要另开一个无边框、置顶、不抢焦点、不进任务栏的小窗口贴着托盘。
///
///   两条通道都有用，不是重复：
///     · tooltip  —— 零窗口，鼠标扫过就知道；
///     · 悬浮窗    —— 需要一直看着的时候（比如盯着剩余额度做任务）。
#[tauri::command]
pub async fn overlay_toggle(app: AppHandle, visible: bool) -> Result<bool, String> {
    let Some(win) = app.get_webview_window("overlay") else {
        return Err("悬浮窗不存在（配置里未声明）".to_string());
    };
    if visible {
        // 先按托盘位置摆好再显示 —— 反过来会看到窗口从上次的位置跳过去。
        position_overlay(&app, &win);
        win.show().map_err(|err| err.to_string())?;
    } else {
        win.hide().map_err(|err| err.to_string())?;
    }
    Ok(visible)
}

/// 把悬浮窗摆到托盘图标附近。
///
/// 托盘图标的位置只能从 `TrayIconEvent` 里拿到（Windows 不提供
/// "查询托盘图标位置"的 API）。所以这里的兜底策略是**贴主屏右下角**：
/// 那是任务栏通知区域的默认位置，绝大多数用户没改过任务栏位置。
/// 做不到精确对齐，但至少每次都出现在同一个可预期的位置。
fn position_overlay(app: &AppHandle, win: &tauri::WebviewWindow) {
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    // `rect()` 返回的是托盘图标的屏幕矩形。它在部分 Windows 版本上
    // 会返回 None（图标还在，位置拿不到）—— 所以必须兜底。
    if let Ok(Some(rect)) = tray.rect() {
        let pos = rect.position.to_physical::<i32>(win.scale_factor().unwrap_or(1.0));
        let size = rect.size.to_physical::<i32>(win.scale_factor().unwrap_or(1.0));
        let win_size = win
            .outer_size()
            .map(|s| (s.width as i32, s.height as i32))
            .unwrap_or((260, 120));
        // 悬浮窗放在图标**上方**（任务栏通常在屏幕底部），水平居中于图标。
        let x = pos.x + size.width / 2 - win_size.0 / 2;
        let y = pos.y - win_size.1 - 8;
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y.max(0)));
        return;
    }

    // 兜底：主显示器右下角，留出任务栏的高度余量。
    if let Ok(Some(monitor)) = win.primary_monitor() {
        let scale = win.scale_factor().unwrap_or(1.0);
        let mpos = monitor.position();
        let msize = monitor.size();
        let win_size = win
            .outer_size()
            .map(|s| (s.width as i32, s.height as i32))
            .unwrap_or((260, 120));
        let margin = (12.0 * scale).round() as i32;
        let taskbar = (56.0 * scale).round() as i32;
        let _ = win.set_position(tauri::PhysicalPosition::new(
            mpos.x + msize.width as i32 - win_size.0 - margin,
            mpos.y + msize.height as i32 - win_size.1 - taskbar - margin,
        ));
    }
}

// ---------------------------------------------------------------------------
// 推送与托盘刷新
// ---------------------------------------------------------------------------

/// 把最新快照推给前端，并同步刷新托盘（tooltip + 菜单文案）。
///
/// ★ 这两件事放在同一个函数里，是因为它们的数据源必须一致。
///   如果分开调，一次刷新中前端拿到新数据、托盘还是旧数据，
///   用户会看到"窗口里显示 45%、托盘里显示 80%"这种自相矛盾的画面。
fn publish(app: &AppHandle, state: &std::sync::Arc<InsightState>) {
    let snapshot = state.snapshot();

    // 推送失败只写日志：前端可能还没加载完（`listen` 尚未注册），
    // 这是正常时序，不是错误 —— 前端挂载时会主动调 `insight_current` 拉一次。
    if let Err(err) = app.emit(EVENT_UPDATE, snapshot.clone()) {
        crate::log::log(format!("推送界面信息失败: {err}"));
    }

    // 托盘刷新失败同样只记日志：托盘不是主功能，它坏了不该影响计时。
    if let Err(err) = crate::tray::sync_tray(app, &snapshot) {
        crate::log::log(format!("刷新托盘信息失败: {err}"));
    }
}

/// 启动后台刷新任务。`setup()` 里调用一次。
///
/// ★ 两个任务、两个周期，**不用一个循环跑两件事**：
///
///   蓝牙 30 秒一次、额度 5 分钟一次。合成一个循环就得取最小公倍数
///   或者维护"第几次循环该做哪件事"的计数器 —— 两者的可读性都更差，
///   而且一个任务卡住会连带另一个不刷新（比如蓝牙适配器抽风卡 3 秒，
///   额度查询就被推迟 3 秒）。分开跑，互不影响。
///
/// ★ 两个任务都用 `loop { ... ; sleep }` 的串行结构，而不是
///   `interval` + 并发：单次查询慢（最长 8 秒超时）时，
///   串行结构天然不会堆积请求 —— 上一轮没回来就不会发下一轮。
///   用 interval 的话，服务端慢下来时我们会不断发新请求，
///   把"慢"放大成"雪崩"。
pub fn spawn_refresh_tasks(app: AppHandle, state: std::sync::Arc<InsightState>) {
    spawn_bluetooth_task(app.clone(), state.clone());
    spawn_quota_task(app, state);
}

/// 蓝牙定时刷新。
fn spawn_bluetooth_task(app: AppHandle, state: std::sync::Arc<InsightState>) {
    tauri::async_runtime::spawn(async move {
        // 第一轮**立刻**跑，不等 30 秒。
        //
        // 理由：用户打开窗口就期待看到设备列表，等 30 秒才出现
        // 会被当成"这个功能坏了"。启动时的那一次扫描是关键路径。
        //
        // 加一点点延迟（800ms）是为了让启动动画和数据库初始化先跑完 ——
        // 蓝牙扫描要占 1–3 秒的 CPU 与适配器，和启动高峰期抢资源
        // 会把首屏拖慢。这个延迟是**体验**上的取舍，不是正确性要求。
        sleep(Duration::from_millis(800)).await;

        loop {
            let inner = state.clone();
            // 扫描在阻塞线程上跑（WinRT），这里 await 它。
            match tauri::async_runtime::spawn_blocking(move || inner.devices.refresh()).await {
                Ok(status) => {
                    state.set_bluetooth(status);
                    publish(&app, &state);
                }
                // 任务 panic / 运行时关闭。记一笔然后继续下一轮 ——
                // 一次扫描失败不该让定时器永久停摆（否则用户会看到
                // 蓝牙信息永远停在某一刻，而日志里只有一开始那条错误）。
                Err(err) => crate::log::log(format!("蓝牙刷新任务异常（继续下一轮）: {err}")),
            }
            sleep(crate::insight::BT_REFRESH).await;
        }
    });
}

/// 额度定时刷新。
fn spawn_quota_task(app: AppHandle, state: std::sync::Arc<InsightState>) {
    tauri::async_runtime::spawn(async move {
        // 比蓝牙再晚一点（1.5 秒）：让首屏优先。
        sleep(Duration::from_millis(1500)).await;

        loop {
            let config = state.quota_config();
            if config.is_ready() {
                let status =
                    tauri::async_runtime::spawn_blocking(move || quota::fetch_blocking(&config))
                        .await
                        .unwrap_or_else(|err| QuotaStatus::Network {
                            message: format!("额度查询任务异常：{err}"),
                        });
                state.set_quota(status);
                publish(&app, &state);
            }
            // 未配置时不发请求，但**依然要走到 sleep** ——
            // 否则会变成死循环里的空转（不 sleep 的 loop 会吃满一个核心）。
            sleep(crate::insight::QUOTA_REFRESH).await;
        }
    });
}

/// 异步睡眠。
///
/// ★ 为什么不直接 `tokio::time::sleep`：`src-tauri` **没有**把 tokio 列为
///   直接依赖（Tauri 内部用它，但没 re-export 时间模块）。加一个
///   `tokio = { version = "1", features = ["time"] }` 只为调一个 sleep，
///   会让 Cargo.toml 上多一条未来可能和 Tauri 内部版本冲突的约束。
///   这里用一个 `futures` 风格的简易实现：把 `sleep` 委托给
///   `tauri::async_runtime` 提供的运行时。
///
///   实现方式是「阻塞线程睡 + 异步等待」的分离：`spawn_blocking` 里做
///   真正的 `thread::sleep`，主线程只是 await 它的完成信号。这在语义上
///   等价于异步定时器，而且**绝不会**阻塞 tokio 的工作线程
///   （阻塞发生在专用的阻塞线程池上）。
async fn sleep(duration: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(duration)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_long_key_keeping_head_and_tail() {
        assert_eq!(mask_key("sk-abcdefghijklmn9f2"), "sk-abc…9f2");
    }

    /// 短 Key 必须整体打码 —— 保留头尾会把整个 Key 暴露出去。
    #[test]
    fn fully_masks_short_key() {
        assert_eq!(mask_key("shortkey"), "••••••••");
        assert_eq!(mask_key("12345678901"), "•••••••••••");
    }

    #[test]
    fn empty_key_masks_to_empty() {
        assert_eq!(mask_key(""), "");
        assert_eq!(mask_key("   "), "");
    }

    /// 中文/emoji 之类的多字节 Key：必须按**字符**而不是字节切，
    /// 按字节切会切出半个 UTF-8 码点直接 panic。
    #[test]
    fn mask_handles_multibyte_by_chars() {
        let masked = mask_key("密钥密钥密钥密钥密钥密钥密钥");
        assert!(masked.starts_with("密钥密钥密"));
        assert!(masked.ends_with('钥'));
    }
}
