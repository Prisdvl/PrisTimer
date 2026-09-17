//! 托盘装配与动态文案刷新。
//!
//! 这是从 `lib.rs` 的 `setup_tray` 里拆出来并重做的 —— 原版只处理
//! 「显示 / 开机自启 / 退出」三项静态菜单。现在托盘要承担
//! 需求里的新职责：**窗口最小化时依然展示蓝牙设备与 opencode-go 额度**。
//!
//! 展示走两条通道（两条都要，缺一不可）：
//!
//!   1. **悬浮提示（tooltip）** —— 鼠标悬停托盘图标时出现。
//!      零窗口、零交互成本，后台刷新后自动更新。
//!   2. **右键菜单** —— 把信息做成菜单条目，比 tooltip 更「存在」，
//!      适合"我想确认一下当前状态"的场景。菜单是**动态**的：
//!      设备列表变化（连了新耳机 / 拔了设备）条目跟着增删。
//!
//! ★ 还有第三条通道：独立悬挂的小窗口（`overlay`）——
//!   给「需要持续盯着额度」的场景用，见 `insight::commands::overlay_toggle`。
//!   三者展示的是同一份快照（`crate::insight::InsightSnapshot`），
//!   文案由 `crate::insight` 的 `device_line` / `quota_line` 统一出。

use std::sync::Arc;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::insight::InsightSnapshot;

// ---------------------------------------------------------------------------
// 状态：托盘图标 + 可复用的固定菜单项
// ---------------------------------------------------------------------------

/// Tauri 托管的状态：托盘图标句柄 + 固定菜单项。
///
/// 为什么固定菜单项要存起来而不是每次重建：
///
///   `sync_tray` 每 30 秒（蓝牙刷新周期）就要更新一次菜单。
///   如果每次都从头 `MenuItem::with_id`，等于高频重建菜单树；
///   Windows 上这会让菜单"闪"一下，用户正在操作菜单时还会被打断。
///   拆成「固定项建一次、动态项每次现建」后，重建的开销只剩
///   动态那几条，而且固定项（显示/自启/退出）的勾选态能保持。
pub struct TrayStore {
    /// 托盘图标。更新 tooltip / 换菜单都要它。
    pub icon: Arc<TrayIcon<tauri::Wry>>,
    /// 固定菜单项（蓝牙区标题、额度标题、分隔线、显示/自启/退出）。
    pub fixed: Arc<TrayFixed>,
}

/// 固定菜单项。内置的 `menu()` 重建函数每次都从它出发。
pub struct TrayFixed {
    /// 「蓝牙设备」区标题（不可点）。
    device_head: MenuItem<tauri::Wry>,
    /// 「opencode-go 额度」条目（不可点）。文本会变，但条目本体复用。
    quota_line: MenuItem<tauri::Wry>,
    /// 分隔线：信息区与操作区之间。
    sep: PredefinedMenuItem<tauri::Wry>,
    /// 「显示主窗口」。
    show: MenuItem<tauri::Wry>,
    /// 「开机自启」（勾选态需要保持，所以放固定区）。
    autostart: CheckMenuItem<tauri::Wry>,
    /// 「退出」。
    quit: MenuItem<tauri::Wry>,
}

impl TrayFixed {
    fn new(app: &AppHandle) -> tauri::Result<Self> {
        Ok(Self {
            device_head: MenuItem::with_id(app, "bt-head", "蓝牙设备", false, None::<&str>)?,
            quota_line: MenuItem::with_id(app, "quota-info", "opencode-go 额度", false, None::<&str>)?,
            sep: PredefinedMenuItem::separator(app)?,
            show: MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?,
            autostart: CheckMenuItem::with_id(
                app,
                "autostart",
                "开机自启",
                true,
                crate::autostart_enabled(app),
                None::<&str>,
            )?,
            quit: MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?,
        })
    }

    /// 组装当前时刻的完整菜单。
    ///
    /// ★ 为什么是「重建整份」而不是「只改条目文本」：
    ///
    ///   设备个数会变（连了新耳机 → 条目 +1；拔掉 → −1）。
    ///   「只改文本」做不到增删条目 —— 菜单 API 没有"在第 X 行插入"。
    ///   重建整份是 Tauri 菜单的常规做法：条目少（个位数），
    ///   每 30 秒一次，开销可忽略；Windows 上菜单内容更新是
    ///   原地重绘的，没有闪烁。
    fn assemble(&self, app: &AppHandle, snapshot: &InsightSnapshot) -> tauri::Result<Menu<tauri::Wry>> {
        // ---- 蓝牙设备区的动态条目 ----
        //
        // ★ 为什么先收集进 `owned` 再统一造引用：
        //   `Menu::with_items` 要的是 `&[&dyn IsMenuItem]`，而动态条目
        //   是在循环里创建的局部变量 —— 循环体每迭代一次就 drop 一次，
        //   直接把 `&item` 塞进引用数组会在借用检查器那里当场暴毙
        //   （"borrowed value does not live long enough"）。
        //   先把所有权收进 Vec，等循环结束、所有条目都活着的时候
        //   再统一转引用，借用检查器就满意了。
        let mut device_lines: Vec<MenuItem<tauri::Wry>> = Vec::new();
        for (index, line) in crate::insight::tray_device_items(&snapshot.bluetooth)
            .into_iter()
            .enumerate()
        {
            // 纯展示条目：不能点、无快捷键 —— 就是一块"文字牌"。
            device_lines.push(MenuItem::with_id(
                app,
                format!("bt-device-{index}"),
                line,
                false,
                None::<&str>,
            )?);
        }

        // ---- 额度区（复用固定条目，只改文本）----
        let quota_text = format!("opencode-go  ·  {}", crate::insight::quota_line(&snapshot.quota));
        self.quota_line.set_text(quota_text)?;

        // ---- 组装引用数组 ----
        let mut items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = Vec::with_capacity(4 + device_lines.len());
        items.push(&self.device_head);
        for item in &device_lines {
            items.push(item);
        }
        items.push(&self.quota_line);
        items.push(&self.sep);
        items.push(&self.show);
        items.push(&self.autostart);
        items.push(&self.quit);

        Menu::with_items(app, &items)
    }
}

// ---------------------------------------------------------------------------
// 装配
// ---------------------------------------------------------------------------

/// 初始托盘装配（`setup()` 里调用一次）。
pub fn build_tray(app: &AppHandle, snapshot: &InsightSnapshot) -> tauri::Result<()> {
    let fixed = Arc::new(TrayFixed::new(app)?);
    let menu = fixed.assemble(app, snapshot)?;

    // ★ 托盘图标：沿用应用图标，不换主题图片。
    //
    //   图标是「信息板入口」的视觉锚点：换图标会破坏用户对托盘
    //   「这是 PrisTimer」的既有认知（老用户升级后找不着托盘）。
    //   tooltip 与菜单内容已经足够表达「这里是设备/额度状态」，
    //   图标保持稳定更划算。
    let mut builder = TrayIconBuilder::with_id("main")
        // tooltip 里放首帧信息 —— 托盘从出现的第一刻起就是"信息板"，
        // 没有一个"先是纯图标、30 秒后才变成信息板"的空窗期。
        .tooltip(crate::insight::tooltip_text(snapshot))
        .menu(&menu)
        .show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    let tray = Arc::new(builder
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            // 左键单击 = 弹回主窗口（菜单留给右键）。
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                crate::window::show_main_window(tray.app_handle());
            }
        })
        .build(app)?);

    app.manage(TrayStore {
        icon: tray.clone(),
        fixed,
    });

    // 通知前端：托盘已就绪，可以调 `overlay_toggle`（悬浮信息窗）了。
    let _ = app.emit("insight:tray-ready", ());

    Ok(())
}

/// 后台刷新完成后调用：把最新快照写进托盘。
///
/// 干两件事：
///   1. `set_tooltip` —— 悬浮提示换成最新状态；
///   2. `set_menu`   —— 菜单重建（设备条目 + 额度条目跟随数据）。
///
/// ★ 更新失败**只写日志**：托盘是展示通道，不是功能通道，
///   它坏了不该拖垮计时器或数据库，也不该让调用方（后台任务）
///   因为一次菜单更新失败而中断刷新循环。
pub fn sync_tray(app: &AppHandle, snapshot: &InsightSnapshot) -> tauri::Result<()> {
    let Some(store) = app.try_state::<TrayStore>() else {
        return Ok(()); // 托盘还没建好（启动早期），首帧快照会由 build_tray 直接渲染
    };

    // ① 悬浮提示。
    store
        .icon
        .set_tooltip(Some(crate::insight::tooltip_text(snapshot)))?;
    // ② 菜单。
    let menu = store.fixed.assemble(app, snapshot)?;
    // ★ 传值而不是传引用：`TrayIcon::set_menu` 的签名是
    //   `set_menu<M: ContextMenu>(&self, menu: Option<M>)` ——
    //   传 `Some(&menu)` 会要求 `&Menu` 实现 `ContextMenu`（没有），
    //   编译报错「trait not satisfied」。
    store.icon.set_menu(Some(menu))?;

    Ok(())
}

/// 菜单事件分发。与旧 `lib.rs::setup_tray` 的逻辑一致。
fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id.as_ref() {
        "show" => crate::window::show_main_window(app),
        "autostart" => {
            let manager = app.autolaunch();
            let enable = !manager.is_enabled().unwrap_or(false);
            let ok = if enable {
                manager.enable().is_ok()
            } else {
                manager.disable().is_ok()
            };
            if ok {
                // 勾选态跟随实际结果（enable/disable 返回 Ok 才算数）。
                if let Some(store) = app.try_state::<TrayStore>() {
                    let _ = store.fixed.autostart.set_checked(enable);
                }
            } else {
                crate::log::log("切换开机自启失败");
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}