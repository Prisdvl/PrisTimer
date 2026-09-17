//! 桌面外壳「去浏览器化」：把 WebView2 里那些**一眼就暴露内核**的行为关掉。
//!
//! ★ 要解决的问题（用户原话：「至少右键或者快捷键不会像浏览器一样」）：
//!
//!   默认的 WebView2 **就是一个 Edge 浏览器内核**，它带着全套浏览器习惯：
//!
//!   | 操作 | 默认行为 | 后果 |
//!   |---|---|---|
//!   | 右键 | 弹出 Edge 的网页菜单（刷新 / 检查 / 另存为…） | 最明显的"这是个网页"信号 |
//!   | F5 / Ctrl+R | 重新加载页面 | 计时器被重置 |
//!   | F12 / Ctrl+Shift+C | 打开开发者工具 | 直接暴露调试面 |
//!   | Ctrl+P | 打印预览 | 整个窗口变成"打印网页" |
//!   | Ctrl+F | 查找栏 | 冒出浏览器查找条 |
//!   | Ctrl +/- / Ctrl+滚轮 | 页面缩放 | 布局被改花且无法恢复 |
//!   | Alt+← / Alt+→ | 前进后退 | 计时器被导航走 |
//!   | 拖拽选中文字 | 蓝色选区 + 拖影 | 网页的文本选择行为 |
//!
//! ★ 三层防线（缺一不可，因为它们管的范围不一样）：
//!
//!   1. **WebView2 原生设置**（本模块）—— 这是唯一能真正关掉
//!      F5/Ctrl+R/Ctrl+P/Ctrl+F/F12/缩放/前进后退的地方。
//!      Chromium 的命令行开关管不到这些加速键，前端 JS 也只能"事后补救"
//!      （事件到达时不执行默认行为），而原生设置是**从根上禁用**。
//!
//!   2. **菜单与 UI 元素**（本模块 + wry 默认值）—— 右键菜单、状态栏、
//!      默认拖拽行为。
//!
//!   3. **前端事件拦截**（App.vue）—— 兜底 + 处理那些原生设置管不到的
//!      （文本编辑类快捷键、拖放文件、双击选中等）。
//!
//! ★ 为什么必须绕到 webview2-com 这一层：
//!
//!   Tauri 的 `WebviewWindowBuilder` 只暴露 `additional_browser_args`
//!   （Chromium 命令行开关），**没有**暴露 WebView2 的 Settings 接口。
//!   而 `AreBrowserAcceleratorKeysEnabled` 这个属性只能通过
//!   `ICoreWebView2Settings3` 设置 —— 没有任何命令行开关等价于它。
//!
//!   wry 其实会调它，但**只在 `browser_accelerator_keys == false` 时**，
//!   而它的默认值是 `true`（跟随 WebView2 默认行为）。Tauri 也没把
//!   这个开关透出来。所以只能自己在窗口建好后补一刀。
//!
//! ★ 调用时机很关键：必须是**页面导航完成之后**。
//!   WebView2 的 Settings 在每次导航后会重置回环境默认值，所以在
//!   `setup` 里设一次是不够的 —— 见 `harden_webview` 的注释。

use tauri::{AppHandle, Manager};

/// 对指定窗口做「去浏览器化」加固。幂等，可重复调用。
///
/// 用 `with_webview` 而不是直接拿 HWND 去 `FindWindow` 找 WebView2：
/// 前者由 Tauri/wry 给出**当前窗口自己的**控制器，后者要靠窗口类名去猜
/// （`Chrome_WidgetWin_1` 之类），在同时存在多个 WebView2 窗口
/// （我们还有 overlay 悬浮窗）时会拿错对象。
#[cfg(windows)]
pub fn harden_webview(app: &AppHandle, label: &str) {
    let Some(window) = app.get_webview_window(label) else {
        crate::log::log(format!("加固 WebView 外壳失败：窗口 {label} 不存在"));
        return;
    };

    // `with_webview` 的回调在**主线程**上执行（它要访问 WebView2 的
    // 控制器，而控制器是线程亲和的）。回调是 FnOnce + Send，无需 'static
    // 捕获外部状态 —— 这里只用 label 记日志。
    let label_owned = label.to_string();
    let result = window.with_webview(move |platform| {
        apply_hardening(&platform, &label_owned);
    });

    if let Err(err) = result {
        // 加固失败不该让应用起不来：最坏情况就是用户还能按 F5。
        // 这是"体验降级"，不是"功能损坏"。
        crate::log::log(format!("加固 WebView 外壳失败（{label}）：{err}"));
    }
}

#[cfg(not(windows))]
pub fn harden_webview(_app: &AppHandle, _label: &str) {
    // 非 Windows 平台的 WebView（WKWebView / WebKitGTK）没有这组设置，
    // 由前端的事件拦截兜底。
}

/// 真正设置 WebView2 属性的地方。
///
/// 三组设置各自有对应的 Settings 版本，必须 `cast` 到足够新的接口 ——
/// cast 失败（WebView2 运行时版本太老）时**静默跳过那一项**而不是整体
/// 失败：老运行时上能关多少算多少，总比什么都不关强。
#[cfg(windows)]
fn apply_hardening(platform: &tauri::webview::PlatformWebview, label: &str) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Settings3, ICoreWebView2Settings4, ICoreWebView2Settings5,
        ICoreWebView2Settings6,
    };
    use windows::core::Interface;

    let controller = platform.controller();
    // 拿到 webview 对象，再从它取 Settings。
    let webview = match unsafe { controller.CoreWebView2() } {
        Ok(wv) => wv,
        Err(err) => {
            // 窗口刚建好、CoreWebView2 还没就绪时会走到这里。
            // 调用方（`spawn_hardening`）会在页面加载完成后重试。
            crate::log::log(format!("加固 WebView 外壳：CoreWebView2 尚未就绪（{label}）：{err}"));
            return;
        }
    };
    let settings = match unsafe { webview.Settings() } {
        Ok(s) => s,
        Err(err) => {
            crate::log::log(format!("加固 WebView 外壳：取 Settings 失败（{label}）：{err}"));
            return;
        }
    };

    // ---- ① 浏览器专属加速键（最关键的一项）----
    //
    // 关掉之后，下列按键**在 WebView2 内部就被吞掉**，根本不会到达页面：
    //   Ctrl+F / F3      查找
    //   Ctrl+P           打印
    //   Ctrl+R / F5      重新加载
    //   Ctrl+加号/减号   缩放
    //   Ctrl+Shift+C     开发者工具
    //   F12              开发者工具
    //   Alt+←/→、Back/Forward 导航
    //
    // 这一项是"看起来不像浏览器"的核心：没有它，用户随手一个 F5
    // 页面就重载、计时器归零，而 Ctrl+P 会弹出打印预览把整个应用
    // 变成一个"网页打印"界面。
    if let Ok(settings3) = settings.cast::<ICoreWebView2Settings3>() {
        // SAFETY: settings3 是有效的 COM 接口；传 FALSE 是文档定义的用法。
        if let Err(err) = unsafe { settings3.SetAreBrowserAcceleratorKeysEnabled(false) } {
            crate::log::log(format!("关闭浏览器加速键失败（{label}）：{err}"));
        }
    } else {
        crate::log::log("WebView2 运行时过旧，无法关闭浏览器加速键");
    }

    // ---- ② 缩放控制 ----
    //
    // 关掉 Ctrl+滚轮 / 捏合缩放。缩放一旦发生是**持久化**的
    // （WebView2 按 origin 记住 zoom factor），用户不小心放大一次，
    // 之后每次打开都是放大的，界面看起来就是"坏掉了"。
    if let Err(err) = unsafe { settings.SetIsZoomControlEnabled(false) } {
        crate::log::log(format!("关闭缩放控制失败（{label}）：{err}"));
    }
    // 顺手把当前缩放钉回 100%：万一之前被改过（老版本没关这个开关），
    // 这次启动就恢复正常。
    let _ = unsafe { controller.SetZoomFactor(1.0) };

    // ---- ③ 右键菜单 ----
    //
    // 前端已经拦了 `contextmenu` 事件，但那是"事件到达 JS 之后"才拦的 ——
    // WebView2 的默认菜单在某些路径（比如长按、某些输入法场景）仍可能
    // 抢在前头。这里从**原生层**关掉，双保险。
    if let Err(err) = unsafe { settings.SetAreDefaultContextMenusEnabled(false) } {
        crate::log::log(format!("关闭默认右键菜单失败（{label}）：{err}"));
    }

    // ---- ④ 状态栏 / 开发者工具 ----
    if let Err(err) = unsafe { settings.SetIsStatusBarEnabled(false) } {
        crate::log::log(format!("关闭状态栏失败（{label}）：{err}"));
    }
    // ★ debug 构建保留开发者工具：开发时按 F12 打不开会很难受，
    //   而 release 必须关掉 —— 那是最直接的"这是个网页"的证据，
    //   也让人能随意改 DOM 绕过界面约束。
    //   （wry 自己也遵循同样的规则：debug 下 devtools 恒为 true。）
    #[cfg(not(debug_assertions))]
    {
        if let Err(err) = unsafe { settings.SetAreDevToolsEnabled(false) } {
            crate::log::log(format!("关闭开发者工具失败（{label}）：{err}"));
        }
    }

    // ---- ⑤ 表单自动填充 ----
    //
    // 输入 API Key 时弹出"保存密码/自动填充"的浏览器气泡，非常出戏。
    if let Ok(settings4) = settings.cast::<ICoreWebView2Settings4>() {
        if let Err(err) = unsafe { settings4.SetIsGeneralAutofillEnabled(false) } {
            crate::log::log(format!("关闭自动填充失败（{label}）：{err}"));
        }
        let _ = unsafe { settings4.SetIsPasswordAutosaveEnabled(false) };
    }

    // ---- ⑥ 滑动导航手势 ----
    //
    // 触控板上双指左右滑 = 前进/后退。桌面应用里这会把计时器"滑走"。
    if let Ok(settings5) = settings.cast::<ICoreWebView2Settings5>() {
        let _ = unsafe { settings5.SetIsPinchZoomEnabled(false) };
    }
    if let Ok(settings6) = settings.cast::<ICoreWebView2Settings6>() {
        if let Err(err) = unsafe { settings6.SetIsSwipeNavigationEnabled(false) } {
            crate::log::log(format!("关闭滑动导航失败（{label}）：{err}"));
        }
        // 浏览器内置的错误页（"无法访问此页面"那个 Edge 风格灰白页）
        // 也关掉 —— 万一真出问题，用户看到的是我们自己的界面，
        // 而不是一个暴露内核的 Edge 错误页。
        let _ = unsafe { settings6.SetIsBuiltInErrorPageEnabled(false) };
    }

    crate::log::log(format!("WebView 外壳加固完成（{label}）"));
}

/// 在页面加载完成后加固，并在**每次导航后**重新加固。
///
/// ★ 为什么不能只在 setup 里设一次：
///
///   WebView2 的 Settings 对象在**每次导航后会被重置**回环境默认值
///   （微软文档里对 `AreBrowserAcceleratorKeysEnabled` 明确写了
///   "will be disabled after the next navigation" 这类语义）。
///   我们这个应用的窗口导航很少（基本就是启动那一次），但：
///     · 启动那一刻 `CoreWebView2` 可能还没就绪（它是异步创建的），
///       第一次调用会静默失败；
///     · 开发模式下 Vite 热重载会触发导航。
///   所以每次页面加载完成都重新加固一次 —— 幂等操作，多调几次无副作用。
///
///   这是"设置看起来没生效"这类问题最常见的成因：**时机不对**。
///   加固本身没问题，问题是在 WebView 还没准备好的时候调的。
///
/// 实现方式：轮询 + 页面加载双保险。
///   · 立刻试一次（多数情况下 CoreWebView2 已就绪）；
///   · 再延迟重试两次，覆盖"创建比 setup 慢"的情况。
///   不做 `on_page_load` 回调是因为那需要在**建窗时**注册，
///   而窗口是配置文件里声明的（不是我们代码里建的），
///   挂回调要改成代码建窗 —— 那会丢掉 tauri.conf.json 里那一堆
///   窗口属性，得不偿失。
#[cfg(windows)]
pub fn harden_with_retry(app: AppHandle, label: &'static str) {
    // 立刻一次 + 两个延迟重试。总时长 ~3 秒，足够覆盖 WebView2 的
    // 异步初始化（实测 200–800ms）。
    for delay_ms in [0u64, 1200, 3000] {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if delay_ms > 0 {
                // 用 spawn_blocking 睡，避免占用 tokio 的工作线程
                // （与 insight::commands 里同一个理由）。
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                })
                .await;
            }
            harden_webview(&app, label);
        });
    }
}

#[cfg(not(windows))]
pub fn harden_with_retry(_app: AppHandle, _label: &'static str) {}
