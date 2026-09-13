//! 窗口几何与形态：位置记忆、圆角、迷你壳、分帧动画。
//!
//! P1-⑥ 自 lib.rs 拆出（原文搬迁）。这里既有 #[tauri::command]（win_state_*
//! / set_mini_shell / animate_window_to），也有 setup 装配要调的
//! place_main_window / show_main_window。

use tauri::{AppHandle, Manager};

/// 从托盘恢复主窗口。
pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

// ---------------------------------------------------------------------------
// 窗口几何持久化
//
// 为什么放在 Rust 而不是前端 localStorage：
//   窗口必须「以记忆中的几何直接出现」，而不是先露出默认位置再跳过去。
//   前端要等 WebView2 把页面加载完（实测 7 秒上下，`visible: false` 期间还会
//   被节流拖到 13 秒）才拿得到 localStorage 里的值 —— 那时窗口早被用户看见了。
//   Rust 在 `setup()` 里同步读一个小 JSON、改完几何再 `show()`，几十毫秒完事。
//
// 坐标系（实测，极易踩）：
//   · `set_size` / `inner_size`      = 客户区尺寸
//   · `set_position` / `outer_position` = 外框左上角
//   存和取必须用同一对；混用（存 outer_size 却用 set_size 还原）会让窗口
//   每次重启长大一圈边框 —— 16×9，肉眼看不出来，重启十次就少一块内容。
// ---------------------------------------------------------------------------

/// 迷你组件的客户区尺寸（物理像素 = 该值 × 缩放因子）。
const MINI_W: f64 = 264.0;
const MINI_H: f64 = 96.0;
/// 常规窗口的最小尺寸。不写进 tauri.conf —— 静态最小尺寸会把 264px 的
/// 迷你小窗挡在门外，所以只在常规形态下运行时设置。
const MIN_W: f64 = 760.0;
const MIN_H: f64 = 560.0;

/// 常规形态的几何：客户区尺寸 + 外框左上角。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NormalGeom {
    w: f64,
    h: f64,
    x: i32,
    y: i32,
}

/// 迷你组件的位置（外框左上角）。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MiniPos {
    x: i32,
    y: i32,
}

/// 落盘的窗口状态。字段全可选：老版本文件、手改坏的文件都不该让启动失败。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WinStateFile {
    #[serde(default)]
    normal: Option<NormalGeom>,
    #[serde(default)]
    maximized: bool,
    #[serde(default)]
    mini_mode: bool,
    #[serde(default)]
    mini: Option<MiniPos>,
}

fn win_state_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("window.json"))
}

/// 读窗口状态。文件不存在、损坏、字段缺失一律回默认值 —— 宁可用默认几何，
/// 也不能因为一个坏文件就启动不了。
fn read_win_state(app: &AppHandle) -> WinStateFile {
    win_state_path(app)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_win_state(app: &AppHandle, state: &WinStateFile) -> Result<(), String> {
    let path = win_state_path(app).ok_or_else(|| "找不到应用数据目录".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    let json = serde_json::to_string_pretty(state).map_err(|err| err.to_string())?;
    std::fs::write(&path, json).map_err(|err| err.to_string())
}

/// 位置是否落在某台显示器的可见范围内（留 40px 容差）。
/// 拔掉显示器、改过分辨率之后，存下的位置可能已经跑到屏幕外 —— 那就别恢复。
fn position_on_screen(win: &tauri::WebviewWindow, x: i32, y: i32, w: i32, h: i32) -> bool {
    let Ok(monitors) = win.available_monitors() else {
        return false;
    };
    monitors.iter().any(|m| {
        let pos = m.position();
        let size = m.size();
        let (mw, mh) = (size.width as i32, size.height as i32);
        x + w > pos.x + 40 && x < pos.x + mw - 40 && y + h > pos.y + 40 && y < pos.y + mh - 40
    })
}

/// 给无边框窗口补上 DWM 圆角。
///
/// `decorations: false` 的窗口没有 `WS_CAPTION`，Windows **不会**给它自动圆角 ——
/// 从系统角度看它就是个矩形，四个直角在深色桌面上显得很生硬（尤其迷你组件那种
/// 小尺寸窗口，直角几乎占满了视觉重量）。
///
/// `DWMWA_WINDOW_CORNER_PREFERENCE` 是 Win11（Build 22000+）才支持的属性，
/// 更老的系统会直接返回失败 —— 忽略即可，退回直角不影响任何功能。
#[cfg(windows)]
fn apply_round_corners(win: &tauri::WebviewWindow, _small: bool) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    let Ok(hwnd) = win.hwnd() else {
        return;
    };
    // 迷你组件与常规窗口统一用标准半径（DWMWCP_ROUND ≈ 8px）——
    // 之前小组件用 ROUNDSMALL（≈4px），在 264×96 的窗口上圆感几乎不可见，
    // 用户明确要求迷你组件也是圆角。
    let preference: i32 = DWMWCP_ROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd.0,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &preference as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

#[cfg(not(windows))]
fn apply_round_corners(_win: &tauri::WebviewWindow, _small: bool) {}

/// 按记忆的几何摆好窗口，然后 `show()` 出来。
///
/// 必须在窗口被用户看见之前调用，且这是**唯一**的启动期几何来源 ——
/// 前端不再参与恢复，也就没有「先默认位置、再跳过去」这一出。
pub(crate) fn place_main_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let state = read_win_state(app);
    apply_round_corners(&win, state.mini_mode);

    if state.mini_mode {
        // 迷你组件：固定客户区尺寸 + 置顶 + 贴边定位 + 关阴影（不可见 frame 黑框）
        let scale = win.scale_factor().unwrap_or(1.0);
        let _ = win.set_shadow(false);
        let pw = (MINI_W * scale).round() as u32;
        let ph = (MINI_H * scale).round() as u32;
        let _ = win.set_resizable(false);
        let _ = win.set_min_size(None::<tauri::LogicalSize<f64>>);
        let _ = win.set_size(tauri::PhysicalSize::new(pw, ph));
        let _ = win.set_always_on_top(true);

        // 位置是外框左上角，贴边就得用外框尺寸 —— 拿客户区宽度去算，
        // 组件右边缘会探出屏幕（264 + 14 的边距配 280 的外框，正好越界 2px）。
        let (ow, oh) = win
            .outer_size()
            .map(|s| (s.width as i32, s.height as i32))
            .unwrap_or((pw as i32, ph as i32));
        let placed = state
            .mini
            .filter(|m| position_on_screen(&win, m.x, m.y, ow, oh))
            .map(|m| {
                let _ = win.set_position(tauri::PhysicalPosition::new(m.x, m.y));
            })
            .is_some();
        if !placed {
            if let Ok(Some(monitor)) = win.current_monitor() {
                let margin = (14.0 * scale).round() as i32;
                let mpos = monitor.position();
                let msize = monitor.size();
                let _ = win.set_position(tauri::PhysicalPosition::new(
                    mpos.x + msize.width as i32 - ow - margin,
                    mpos.y + msize.height as i32 - oh - margin,
                ));
            }
        }
    } else {
        let _ = win.set_shadow(true);
        let _ = win.set_resizable(true);
        let _ = win.set_min_size(Some(tauri::LogicalSize::new(MIN_W, MIN_H)));
        if let Some(n) = state.normal {
            if n.w >= 400.0
                && n.h >= 300.0
                && position_on_screen(&win, n.x, n.y, n.w as i32, n.h as i32)
            {
                let _ = win.set_size(tauri::PhysicalSize::new(n.w as u32, n.h as u32));
                let _ = win.set_position(tauri::PhysicalPosition::new(n.x, n.y));
            }
        }
        if state.maximized {
            let _ = win.maximize();
        }
    }

    let _ = win.show();
    let _ = win.set_focus();
}

/// 合并式写入窗口状态：只更新传进来的字段，其余保留。
///
/// 合并而不是整体覆盖，是因为调用方各自只知道自己那一部分 —— 移动窗口时
/// 只知道常规几何，切迷你时只知道形态标志，整体覆盖会把对方的信息抹掉。
#[tauri::command]
pub(crate) fn win_state_save(
    app: AppHandle,
    normal: Option<NormalGeom>,
    maximized: Option<bool>,
    mini_mode: Option<bool>,
    mini: Option<MiniPos>,
) -> Result<WinStateFile, String> {
    let mut state = read_win_state(&app);
    if let Some(n) = normal {
        state.normal = Some(n);
    }
    if let Some(m) = maximized {
        state.maximized = m;
    }
    if let Some(b) = mini_mode {
        state.mini_mode = b;
        // 圆角半径跟着形态走：小组件用 ROUNDSMALL，常规窗口用 ROUND。
        if let Some(win) = app.get_webview_window("main") {
            apply_round_corners(&win, b);
        }
    }
    if let Some(p) = mini {
        state.mini = Some(p);
    }
    write_win_state(&app, &state)?;
    Ok(state)
}

/// 读当前窗口状态（前端挂载时对齐用）。
#[tauri::command]
pub(crate) fn win_state_get(app: AppHandle) -> WinStateFile {
    read_win_state(&app)
}

/// 迷你形态的窗口属性开关：一次 IPC 完成 4 项设置。
///
/// 之前前端逐项 set_resizable / set_min_size / set_always_on_top / set_shadow，
/// 每次实测 ~21ms，4 次串行在动画关键路径上白占 ~80ms（内容已淡出、窗口干等）。
#[tauri::command]
pub(crate) async fn set_mini_shell(app: AppHandle, mini: bool) -> Result<(), String> {
    use tauri::Manager;
    let win = app.get_webview_window("main").ok_or("主窗口不存在")?;
    if mini {
        win.set_resizable(false).map_err(|e| e.to_string())?;
        win.set_min_size::<tauri::LogicalSize<f64>>(None)
            .map_err(|e| e.to_string())?;
        win.set_always_on_top(true).map_err(|e| e.to_string())?;
        // ★ 迷你态关 DWM 阴影：shadow=true 的不可见 resize frame 是"黑框"来源
        win.set_shadow(false).map_err(|e| e.to_string())?;
    } else {
        win.set_always_on_top(false).map_err(|e| e.to_string())?;
        win.set_shadow(true).map_err(|e| e.to_string())?;
        win.set_min_size(Some(tauri::LogicalSize::new(760.0, 560.0)))
            .map_err(|e| e.to_string())?;
        win.set_resizable(true).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 原生窗口几何动画：把"迷你↔常规"的过渡从 JS rAF 挪到 Rust 侧分帧执行。
///
/// 为什么必须挪：旧实现每帧调 `setSize` + `setPosition` 两次 IPC，WebView2 桥
/// 单程 2–6ms、两次再加 await 排队，一帧 60fps 的 16ms 预算被吃光 —— 观感卡顿。
/// 这里每帧只是一次本地 Win32 调用（微秒级），节奏由 `Instant` 驱动，与调用方
/// 帧率解耦；曲线仍是 easeOutCubic，观感一致。
///
/// 尺寸语义与前端 `set_size` 对齐：参数是**客户区**物理尺寸；无边框窗口的
/// 客户区原点即外框原点，外框比客户区多出的部分（shadow=true 时的不可见
/// resize frame）全部落在右侧/下方，按帧把差值加回外框即可。
#[tauri::command]
pub(crate) async fn animate_window_to(
    app: AppHandle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    duration_ms: u64,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::time::{Duration, Instant};
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::Graphics::Dwm::DwmFlush;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetClientRect, GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
        };
        let win = app.get_webview_window("main").ok_or("主窗口不存在")?;
        // HWND 裸指针不能跨线程搬，转成数值再在阻塞线程里还原
        let hwnd_val = win.hwnd().map_err(|e| e.to_string())?.0 as isize;
        tauri::async_runtime::spawn_blocking(move || {
            let hwnd = hwnd_val as *mut core::ffi::c_void;
            // SAFETY：hwnd 是本应用主窗口的有效句柄（单实例守卫保证唯一）；
            // GetClientRect/GetWindowRect/SetWindowPos 都是纯查询/几何操作。
            let mut cr = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let mut wr = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if unsafe { GetClientRect(hwnd, &mut cr) } == 0
                || unsafe { GetWindowRect(hwnd, &mut wr) } == 0
            {
                return Err("GetClientRect/GetWindowRect 失败".into());
            }
            let from_w = (cr.right - cr.left) as f64;
            let from_h = (cr.bottom - cr.top) as f64;
            let from_x = wr.left as f64;
            let from_y = wr.top as f64;
            // 外框 − 客户区 的差值（无边框窗全部在右侧/下方）
            let dw = (wr.right - wr.left - (cr.right - cr.left)) as f64;
            let dh = (wr.bottom - wr.top - (cr.bottom - cr.top)) as f64;
                let dur = (duration_ms.max(1) as f64) / 1000.0;
                let t0 = Instant::now();
                loop {
                    let step_start = Instant::now();
                    let t = ((step_start - t0).as_secs_f64() / dur).min(1.0);
                    let e = 1.0 - (1.0 - t).powi(3);
                    let cw = from_w + (w as f64 - from_w) * e;
                    let ch = from_h + (h as f64 - from_h) * e;
                    let ox = from_x + (x as f64 - from_x) * e;
                    let oy = from_y + (y as f64 - from_y) * e;
                    unsafe {
                        SetWindowPos(
                            hwnd,
                            std::ptr::null_mut(),
                            ox.round() as i32,
                            oy.round() as i32,
                            (cw + dw).round() as i32,
                            (ch + dh).round() as i32,
                            SWP_NOACTIVATE | SWP_NOZORDER,
                        );
                    }
                    if t >= 1.0 {
                        break;
                    }
                    // ★ 双重节流：最小步进 15ms（≤67fps）+ DwmFlush 对齐合成器 vsync。
                    //   每次 resize 对 WebView2 都是一次完整的布局+合成事务，实测
                    //   1ms 步进（~200Hz）会把消息泵淹没 —— 页面 rAF 停摆数百 ms、
                    //   尾部多步被合并成跳变（观感就是"卡顿"）。60fps 节流后全动画
                    //   只有 ~19 次事务，间隔充裕，渲染管线零积压。
                    let budget = Duration::from_millis(15);
                    let spent = step_start.elapsed();
                    if spent < budget {
                        std::thread::sleep(budget - spent);
                    }
                    unsafe {
                        let _ = DwmFlush();
                    }
                }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (x, y, w, h);
        use tauri::Manager;
        let win = app.get_webview_window("main").ok_or("主窗口不存在")?;
        let scale = win.scale_factor().map_err(|e| e.to_string())?;
        let _ = scale;
        win.set_size(tauri::PhysicalSize::new(w as u32, h as u32))
            .map_err(|e| e.to_string())?;
        win.set_position(tauri::PhysicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
        let _ = duration_ms;
        Ok(())
    }
}
