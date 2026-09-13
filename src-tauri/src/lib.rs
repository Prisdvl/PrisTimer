use std::sync::{Arc, Mutex};

use pristimer_core::{Command, Phase, PomodoroConfig, SystemClock, TimerRuntime, TimerSnapshot, TimerState};
use pristimer_store::Store;
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_notification::NotificationExt;

mod backup;
mod commands;
pub mod log;
mod pomodoro;
mod recorder;
mod state;
mod window;

use pomodoro::PomodoroManager;
use recorder::Recorder;
use state::{lock, AppTimer, CommandCell, SharedPomodoro, SharedRecorder, SnapshotCache};
use commands::pomodoro::pomodoro_notice;
use window::{place_main_window, show_main_window};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();

    // ★ 日志必须在**任何** Tauri 初始化之前就位。
    //
    // `setup()` 里那句 `log::init` 已经太晚：`App::build` 阶段自己就可能失败
    // （WebView2 环境建不起来、运行时被策略禁用、资源目录不可写）。那条路径上
    // 我们连"原因"都写不出去 —— 而这正是「图标点了，闪一下就没了，日志里
    // 干干净净」的成因之一。用上下文里的 identifier 自己拼出与
    // `app_data_dir()` 一致的位置（Windows 即 `%APPDATA%\<identifier>`）。
    if let Some(dir) = early_data_dir(&context) {
        log::init(&dir);
    }

    // 单实例守卫：必须在 `build_app().run()` 之前 —— Tauri 一旦开始建窗口、
    // 跑 setup，再想「不启动」就已经太晚了（setup 里会去开数据库，而那正是
    // 旧实例锁着的东西）。
    //
    // 但它**不该**比日志更早。守卫自己会写几行关键的日志（"已有实例在运行，
    // 已把它的窗口带到前台" / "窗口尚未就绪"），放在 `log::init` 之前，这些行
    // 会因为日志句柄尚未建立而**静默丢失** —— 而这恰好是排查「双击图标没
    // 反应」时最需要的那几行。`generate_context!` 与 `log::init` 都只碰编译期
    // 内嵌的资源与一个日志文件，不会去动数据库，所以把守卫挪到它们之后是安全的。
    if !acquire_single_instance() {
        return;
    }

    log::log(format!(
        "进程启动 v{} · pid {}",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    ));

    // ★ 兜底：把「进程凭空消失」换成一个日志行 + 一个对话框。
    //
    // Tauri 对 setup 钩子返回 Err 的处理是直接 panic（`app.rs` 里那句
    // "Failed to setup app"）—— 而 GUI 子系统下 panic 信息写向不存在的 stderr，
    // 用户看到的就是「图标点了，什么都没发生」。真实现场是一次
    // `数据库错误: database is locked`：另一个实例正持着写锁，
    // 于是后起的这个在 setup 里失败、进程消失。
    //
    // 触发源已在两处堵住（单实例守卫 + `Store::open` 的重试），这里再兜一层：
    // 今后无论启动阶段因为什么炸掉，用户至少能得到一句人话、我们至少有一行日志。
    // `AssertUnwindSafe` 是必要的：`Context` 不保证 `UnwindSafe`，而我们在这里
    // 明确接受"捕获后立刻退出、不复用任何状态"。
    let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        build_app().run(context)
    }));
    match started {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            log::log(format!("应用异常退出: {err}"));
            show_fatal_dialog(&format!("{err}\n\n{}", where_to_look()));
            std::process::exit(1);
        }
        Err(_) => {
            // panic 的位置与载荷已经由 log.rs 的 panic hook 记下，这里只负责
            // 把它翻译成用户能理解的一句话。
            log::log("启动过程中发生 panic，正在退出");
            show_fatal_dialog(&format!(
                "应用在启动过程中发生内部错误，已退出。\n\n{}",
                where_to_look()
            ));
            std::process::exit(1);
        }
    }
}

/// 推断应用数据目录（与 Tauri 的 `app_data_dir()` 同一处）。
///
/// Windows 上 `app_data_dir()` 就是 `%APPDATA%\<identifier>`；这里提前算一次，
/// 是为了能在 Tauri 还没建起来的时候就把日志打开。
#[cfg(windows)]
fn early_data_dir(context: &tauri::Context) -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(std::path::PathBuf::from(base).join(&context.config().identifier))
}

#[cfg(not(windows))]
fn early_data_dir(_context: &tauri::Context) -> Option<std::path::PathBuf> {
    None
}

/// 告诉用户「去哪儿找原因」。日志还没建立时（问题发生得比 `log::init` 更早）
/// 也如实说，不含糊其辞。
fn where_to_look() -> String {
    match log::path() {
        Some(path) => format!("详细原因见日志：\n{}", path.display()),
        None => "问题发生得太早，日志尚未建立 —— 请检查应用数据目录是否可写。".to_string(),
    }
}

/// 单实例守卫：本进程是否拿到了「唯一实例」的名额。
///
/// 这不只是体面问题。两个实例会各跑一个计时线程、各持一个 SQLite 连接写同一个
/// 库文件：轻则同一条会话被写两遍，重则两个连接同时执行建表 / 切 WAL 时撞锁
/// —— 后起的那个在 setup 钩子里失败，Tauri 直接 panic，**用户在屏幕上看到的
/// 就是"闪一下就没了"**。
///
/// 注意这不是"没设等待"造成的：rusqlite 开连接时已把 `busy_timeout` 设成
/// 5000ms。真正等不来的是 WAL 下的 `SQLITE_BUSY_SNAPSHOT` —— 连接先读
/// pragma 拿到读快照、再想升级为写，而快照已被另一实例的提交作废；SQLite
/// 对这种冲突**刻意不调用忙等处理器**，等 5 秒也一样立刻失败。所以这里必须
/// 从源头不产生第二个实例，而不是指望重试能等过去。
/// 而应用启动要 4–6 秒，等不及再点一次图标是极常见的操作。
///
/// 用的是「进程级命名互斥体」这把最轻的锁：拿到即持有一辈子（不显式释放，
/// 进程退出时由系统回收）。第二个实例发现自己没拿到名额，就把已有窗口捞到
/// 前台然后退出 —— 用户感受到的是「点了图标，窗口出来了」。
#[cfg(windows)]
fn acquire_single_instance() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::{CreateMutexW, Sleep};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };

    let name: Vec<u16> = "Local\\com.pristimer.app.single-instance\0"
        .encode_utf16()
        .collect();
    // SAFETY: 三个调用都是「传一个合法的 UTF-16 以 NUL 结尾的串」这种直白用法；
    // 返回的句柄故意不关 —— 它就是这名额的凭证，关了等于把名额还回去。
    // （windows-sys 0.61 起 `HANDLE` / `HWND` 是裸指针，判空用 `is_null()`。）
    unsafe {
        let guard = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if guard.is_null() {
            // 连互斥体都建不出来（极端受限环境）：不拿它当理由拦住启动
            log::log("单实例互斥体创建失败，按「允许启动」处理");
            return true;
        }
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return true;
        }

        // 已有实例。这里**不能**直接退出走人：本应用启动要 4–6 秒（WebView2
        // 初始化），用户"没看见窗口所以又点了一次"是最常见的重复启动时机 ——
        // 那一刻已有实例的窗口可能还不存在。所以给它一点时间，等窗口出现后
        // 再把它捞到前台，让用户的第二次点击真的"有反应"。
        let title: Vec<u16> = "PrisTimer\0".encode_utf16().collect();
        for _ in 0..40 {
            // 40 × 250ms ≈ 10 秒
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if !hwnd.is_null() {
                // 托盘最小化过就 restore，再置顶（无边框窗口没有 WS_CAPTION，
                // 但 SW_RESTORE/SW_SHOW 一样有效）。
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                log::log("已有实例在运行，已把它的窗口带到前台，本次启动退出");
                return false;
            }
            Sleep(250);
        }
        log::log("已有实例在运行（窗口尚未就绪），本次启动退出");
        false
    }
}

#[cfg(not(windows))]
fn acquire_single_instance() -> bool {
    true
}

/// 把「启动失败」摆到用户眼前。
///
/// 之所以要专门弹窗：这个应用是无边框 + 隐藏控制台的 GUI 进程，它没有任何
/// 可见的失败出口 —— 数据库打不开、磁盘满、权限被拒，用户看到的统一是
/// 「我点了，什么都没发生」。既然已经到了要退出这一步，至少把原因写清楚。
#[cfg(windows)]
fn show_fatal_dialog(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

    let text: Vec<u16> = format!("{message}\0").encode_utf16().collect();
    let caption: Vec<u16> = "PrisTimer 启动失败\0".encode_utf16().collect();
    // SAFETY: 两个参数都是合法的 NUL 结尾 UTF-16 串；hwnd 传空表示无属主窗口。
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_fatal_dialog(message: &str) {
    eprintln!("PrisTimer 启动失败: {message}");
}

fn build_app() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        // 关窗 = 隐藏到托盘，后台继续计时；真正退出走托盘菜单的「退出」。
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .setup(|app| {
            // 1. 打开数据库。放到应用数据目录，各平台会自动定位到合适位置
            //    （Windows: %APPDATA%\com.pristimer.app，macOS: ~/Library/Application Support）。
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            // 日志通常已经在 `run()` 开头就位了（那里才能覆盖 Tauri 自身初始化
            // 失败的路径）。这里再调一次是幂等的兜底 —— 非 Windows 平台拿不到
            // 提前推断的目录，靠这一次打开。
            log::init(&dir);
            log::log(format!("setup 就绪 · 数据目录 {}", dir.display()));
            let db_path = dir.join("pristimer.db");
            // 先记一笔「准备开库」再开：万一 `Store::open` 失败，日志里就能看到
            // 「走到哪一步停的」，而不是只有一条孤零零的 panic。
            log::log(format!(
                "启动 v{} · 数据库 {}",
                env!("CARGO_PKG_VERSION"),
                db_path.display()
            ));
            let store = Store::open(&db_path)?;

            // 番茄配置从 settings 表加载：校验失败（数据被手改坏）就静默回默认值。
            // 必须在 store 移入 Recorder 之前读。
            let saved_config = store
                .get_setting("pomodoro_config")
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_str::<PomodoroConfig>(&raw).ok())
                .filter(|config| config.validate().is_ok());

            // 上次使用的科目标签同样恢复（存的是 JSON，坏数据按无标签处理）。
            let saved_tag = store
                .get_setting("current_tag")
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_str::<Option<String>>(&raw).ok())
                .flatten()
                .filter(|tag| !tag.is_empty());

            // 2. 先做启动恢复。
            let mut recorder = Recorder::new(store);
            let recovered = recorder.recover();

            // 每日首次启动做一份数据库快照（backups/，保留 7 份）。
            // 放在恢复之后：快照里是已恢复归位的最新状态；失败只写日志。
            backup::run_daily_backup(&dir, recorder.store());
            if saved_tag.is_some() {
                recorder.set_tag(saved_tag.clone());
            }

            let recorder: SharedRecorder = Arc::new(Mutex::new(recorder));
            app.manage(recorder.clone());

            // 3. 番茄钟编排器。先 manage 再进计时回调，闭包里才能克隆到它。
            let pomodoro: SharedPomodoro = Arc::new(Mutex::new(match saved_config {
                Some(config) => PomodoroManager::with_config(config),
                None => PomodoroManager::new(),
            }));
            app.manage(pomodoro.clone());

            // 4. 启动计时线程。恢复状态在这里一次性注入 —— 必须在 spawn 时给出，
            //    而不是 spawn 之后再发命令，否则第一帧会先是 Idle / 0，
            //    记录器会把刚恢复出来的会话当成「用户按了重置」结算掉。
            let handle = app.handle().clone();
            let hook_recorder = recorder.clone();
            let hook_pomodoro = pomodoro.clone();
            let cache: SnapshotCache = Arc::new(Mutex::new(None));
            app.manage(cache.clone());
            let hook_cache = cache.clone();
            // 回调里反向下发命令的发送端，spawn 之后立刻填充。
            let commands: CommandCell = Arc::new(Mutex::new(None));
            let hook_commands = commands.clone();

            let runtime = TimerRuntime::spawn_with(
                SystemClock,
                None,
                recovered.as_ref().map(|r| (r.elapsed_ms, r.limit_ms)),
                move |snapshot: TimerSnapshot| {
                    // 先落库，再推送。锁只持有一次 SQL 的时间，不会影响 100ms 的节拍。
                    if let Ok(mut rec) = hook_recorder.lock() {
                        rec.on_snapshot(snapshot);
                    }
                    // 更新缓存，供前端的 timer_current 命令拉取。
                    if let Ok(mut cached) = hook_cache.lock() {
                        *cached = Some(snapshot);
                    }
                    if let Err(err) = handle.emit("timer:update", snapshot) {
                        log::log(format!("推送计时快照失败: {err}"));
                    }

                    // ---- 番茄循环编排：只在「倒计时自然到点」时推进 ----
                    if snapshot.state == TimerState::Finished {
                        let next = hook_pomodoro
                            .lock()
                            .ok()
                            .filter(|p| p.is_enabled())
                            .map(|mut p| p.advance());
                        if let Some((phase, duration_ms)) = next {
                            // 专注落库、休息跳过：必须在命令入队**之前**切开关，
                            // 因为回调与命令处理在同一线程上串行执行。
                            if let Ok(mut rec) = hook_recorder.lock() {
                                rec.set_recording(phase == Phase::Focus);
                            }
                            if let Ok(guard) = hook_commands.lock() {
                                if let Some(tx) = guard.as_ref() {
                                    // 线程已退出时 send 失败 —— 与 runtime.send
                                    // 同一口径：静默忽略，不让计时线程 panic。
                                    let _ = tx.send(Command::Reset);
                                    let _ = tx.send(Command::SetLimit(Some(duration_ms)));
                                    let _ = tx.send(Command::Start);
                                }
                            }
                            let (title, body) = pomodoro_notice(phase, duration_ms);
                            let _ = handle
                                .notification()
                                .builder()
                                .title(title)
                                .body(body)
                                .show();
                        }
                        let status = hook_pomodoro.lock().ok().map(|p| p.status());
                        if let Some(status) = status {
                            if let Err(err) = handle.emit("pomodoro:update", status) {
                                log::log(format!("推送番茄状态失败: {err}"));
                            }
                        }
                    }
                },
            );

            // 填充回调用的命令发送端（mpsc Sender 可克隆，多生产者入队无锁）。
            *lock(&commands) = Some(runtime.sender());
            app.manage(AppTimer(Mutex::new(runtime)));

            // 5. 窗口是 `visible: false` 启动的：这里按记忆的几何摆好再 show 出来。
            //    放在托盘构建**之前** —— 窗口能否出现只依赖这一步，后面任何环节
            //    （托盘、菜单、系统集成）出问题都不会让它变成"启动了却没有窗口"。
            place_main_window(app.handle());

            // 6. 托盘：左键单击恢复窗口，右键菜单（显示 / 开机自启 / 退出）。
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let autostart_item = CheckMenuItem::with_id(
                app,
                "autostart",
                "开机自启",
                true,
                app.autolaunch().is_enabled().unwrap_or(false),
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &autostart_item, &quit_item])?;
            let autostart_for_menu = autostart_item.clone();
            // 图标缺失（打包异常、精简过的资源）不该让托盘构建失败并把整个
            // 启动链带下去 —— 没有图标的托盘总比没有托盘强，日志里留一句就够了。
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("PrisTimer")
                .menu(&menu)
                .show_menu_on_left_click(false);
            match app.default_window_icon() {
                Some(icon) => tray = tray.icon(icon.clone()),
                None => log::log("托盘图标缺失，使用无图标托盘"),
            }
            tray
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "autostart" => {
                        let manager = app.autolaunch();
                        let enable = !manager.is_enabled().unwrap_or(false);
                        let result = if enable {
                            manager.enable()
                        } else {
                            manager.disable()
                        };
                        if result.is_ok() {
                            let _ = autostart_for_menu.set_checked(enable);
                        } else {
                            log::log("切换开机自启失败");
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 左键单击托盘图标 = 弹回主窗口（菜单留给右键）。
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::timer::timer_start,
            commands::timer::timer_pause,
            commands::timer::timer_reset,
            commands::timer::timer_set_limit,
            commands::timer::timer_current,
            commands::timer::recovered_session,
            commands::pomodoro::pomodoro_set,
            commands::pomodoro::pomodoro_current,
            commands::pomodoro::pomodoro_config_get,
            commands::pomodoro::pomodoro_config_set,
            commands::stats::stats_daily,
            commands::stats::stats_summary,
            commands::stats::stats_tags,
            commands::stats::stats_tag_daily,
            commands::stats::stats_hourly,
            commands::tags::tag_set,
            commands::tags::tag_current,
            commands::tags::tag_goals_get,
            commands::tags::tag_goals_set,
            commands::tags::tag_rename,
            commands::export::export_csv,
            commands::export::export_report_csv,
            window::win_state_get,
            window::win_state_save,
            window::set_mini_shell,
            window::animate_window_to
        ])
}

