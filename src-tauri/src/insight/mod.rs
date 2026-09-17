//! 托盘 / 状态栏「环境信息」子系统：蓝牙设备与 opencode-go 额度。
//!
//! 拆成三个子模块，边界按「谁依赖谁」划：
//!
//! ```text
//!   mod.rs      数据结构 + 共享状态（InsightState）+ 快照拼装
//!     ├── device.rs    本机已连接蓝牙设备枚举与电量读取（Windows：WinRT）
//!     ├── quota.rs     opencode-go 额度 HTTP 查询（Windows：WinHTTP）
//!     ├── http.rs      零依赖 HTTPS GET（WinHTTP 薄封装）
//!     └── commands.rs  Tauri 命令 + 后台定时刷新任务 + 托盘刷新
//! ```
//!
//! 托盘本身（`crate::tray`）不在这个模块里：它服务的不止是"环境信息"
//! （还有显示主窗口/开机自启/退出），放进 `insight` 会让依赖方向变乱
//! —— `tray` 消费 `insight` 的数据，而不是反过来。
//!
//! ★ 为什么数据只在这里定义一次：
//!
//! 前端状态栏、托盘悬浮提示、托盘右键菜单**三处消费同一份数据**。
//! 如果各自拼各自的字符串，就会出现「窗口里显示 78%、托盘里显示 78%」
//! 这种看起来一样、改一次文案要改三处的局面。所以这里提供
//! `device_line` / `quota_line` 两个纯函数作为**唯一的文案出处**，
//! 三处展示全部调它。
//!
//! ★ 为什么刷新不在前端做：
//!
//! 需求原文是「不要每次前端 invoke 都重新扫描蓝牙」。除了性能，更硬的
//! 理由是**行为正确性**：经典蓝牙设备的电量要从 SDP 记录里读，Windows
//! 没有缓存，每次查询都要给设备发一次 SDP 请求（1–3 秒、耗设备电量）；
//! 而托盘悬浮提示在窗口最小化时也要显示，那时前端可能已经被系统挂起
//! （`visible: false` 的 WebView2 会被节流），根本推不动刷新。所以刷新
//! 定时器必须活在 Rust 侧的 tokio 任务里，前端只做「读缓存 + 订阅事件」。

pub mod commands;
pub mod device;
pub mod http;
pub mod quota;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// 蓝牙查询两次真实扫描之间的**最小间隔**（节流闸门）。
///
/// 需求里的「蓝牙查询做节流，避免高频扫描卡死蓝牙适配器」就是这条。
/// 取值理由见 `device::DeviceCache` 的注释：10 秒既能让人手点「立即刷新」
/// 有反馈，又远低于会把适配器打到超时的频率。
pub const BT_THROTTLE: std::time::Duration = std::time::Duration::from_secs(10);

/// 蓝牙后台刷新间隔。30 秒是「够新」与「别折腾适配器」的折中：
/// 插拔耳机、开关蓝牙这类事件 30 秒内一定反映到界面上。
pub const BT_REFRESH: std::time::Duration = std::time::Duration::from_secs(30);

/// opencode-go 额度后台刷新间隔。这玩意变化很慢（按小时/天计），
/// 但接口是远端 HTTP，5 分钟一次足够，也不至于对服务端失礼。
pub const QUOTA_REFRESH: std::time::Duration = std::time::Duration::from_secs(300);

/// 单个蓝牙设备的类型。
///
/// 序列化成小写下划线串而不是数字：前端 TypeScript 侧直接写
/// `"classic" | "ble"` 就能拿到完整类型收窄，比 `0 | 1` 可读得多。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    /// 经典蓝牙（BR/EDR）。耳机、音箱、键鼠、车机绝大多数属于这一类。
    Classic,
    /// 低功耗蓝牙（BLE）。手环、手表、部分新鼠标/传感器属于这一类。
    Ble,
    /// 同时具备两种能力（Windows 对部分设备会这样标记）。
    /// 单独列出来而不是硬塞进上面两个，是因为「按 BLE 查电量」和
    /// 「按经典查电量」走的是完全不同的两条路，前端也想知道这一点。
    Dual,
}

impl DeviceKind {
    /// 极短标签，给状态栏那种一行塞好几项的地方用。
    pub fn short(self) -> &'static str {
        match self {
            DeviceKind::Classic => "经典",
            DeviceKind::Ble => "BLE",
            DeviceKind::Dual => "双模",
        }
    }
}

/// 一个已连接的蓝牙设备。
///
/// 字段全部 `#[serde(rename_all = "camelCase")]`：前端拿到的就是
/// `batteryPercent` 而不是 `battery_percent`。这条约定与项目里
/// `TimerSnapshot` 等结构体一致，写错一个字母前端只会拿到 `undefined`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BtDevice {
    /// 稳定标识（Windows 上是 PnP 设备实例 ID，形如
    /// `Bluetooth#BTHENUM#...#<MAC>`）。前端拿它做 `v-for` 的 key —— 
    /// **不要用设备名当 key**：两个同型号耳机同名，列表会错位。
    pub id: String,
    /// 设备名。空名字在 Windows 上是常态（尤其刚连上、名字还没解析出来），
    /// 这里统一兜底成 `未知设备`，免得前端到处写 `|| "未知"`。
    pub name: String,
    /// 设备类型（经典 / BLE / 双模）。
    pub kind: DeviceKind,
    /// 电量百分比 0–100。**读不到就是 `None`** —— 需求明确要求
    /// 「设备无法读取电量时电量返回 null」，不要用 0 冒充，
    /// 0% 和「不知道」在界面上是两件事。
    pub battery_percent: Option<u8>,
    /// 电量的来源，用于排障与界面角标（"这数字是系统给的还是设备报的"）。
    pub battery_source: BatterySource,
    /// 是否为「音频类」设备（耳机/音箱/车机）。
    ///
    /// 独立于 `kind` 存在，因为它是一个**独立维度**：BLE 里有音频设备，
    /// 经典里也有非音频设备（键鼠、串口模块）。Windows 的电量兜底
    /// （PnP 属性 `DEVPKEY_Bluetooth_LastConnectedTime` 那一族）会读
    /// 非音频设备的「电量」属性，那其实是电量计的原始值不是百分比 ——
    /// 所以这个标志决定我们要不要采信系统兜底电量。
    pub is_audio: bool,
}

/// 电量的来源。界面上用它区分「设备真的报了百分比」和「系统兜底估算」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BatterySource {
    /// BLE 标准电池服务（GATT BAS, 0x180F）里的 Battery Level 特征（0x2A19）。
    /// 这是最权威的来源：设备直接报百分比。
    BleBas,
    /// 经典蓝牙的 SDP 记录里带 `Battery Level` 属性（蓝牙规范
    /// `Battery Level`，属性 ID 0x0314）—— Windows 对**已配对**的耳机/
    /// 音箱会在连接时把这条记录缓存在 SDP 数据库里，读它不需要再打扰设备。
    ClassicSdp,
    /// Windows PnP 设备属性里的电量（`{104EA319-6EE2-47D1-BDDB-47A8CA63B19C},10`）。
    /// 这是系统层的兜底：只有音频类设备的值可信（见 `is_audio`）。
    SystemPnp,
    /// 没读到。
    None,
}

impl BatterySource {
    // 来源标签由前端按其业务场景自行取用（devices 数组里带原始枚举值）。
}

/// 蓝牙子系统的整体状态。
///
/// ★ 关键设计：**错误不是「没有数据」，而是数据的一种**。
///
/// 需求要求「蓝牙未开启 / 无已连接设备 / 读取失败时 UI 显示对应提示而不是
/// 崩溃」。最省事的做法是命令返回 `Result`，前端 catch 后显示一句通用错误 ——
/// 但那样 `Err` 就把「未开启」和「适配器驱动炸了」压成了同一句话。这里
/// 把三态明确成枚举，前端一个 `v-if` 就能给出准确文案。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum BtStatus {
    /// 一切正常。`devices` 可能是空数组 —— 空数组是**合法结果**
    /// （蓝牙开着但确实没连东西），不是错误。
    #[serde(rename_all = "camelCase")]
    Ok { devices: Vec<BtDevice> },
    /// 蓝牙无线电被关掉了（硬件开关 / 飞行模式 / Windows 设置里的开关）。
    PoweredOff,
    /// 本机没有蓝牙适配器。
    NoAdapter,
    /// 查询过程本身失败（驱动异常、WinRT 调用返回 HRESULT 错误……）。
    Error { message: String },
}

/// 订阅额度的**单个配额窗口**。
///
/// ★ 为什么需要它（实测）：opencode Go 的用量接口返回的不是一个总额度，
///   而是三个独立重置的窗口：
///
///   ```json
///   {"usage":{
///     "rolling":{"status":"ok","percent":0, "resetsAt":"2026-09-17T11:36:53Z"},
///     "weekly": {"status":"ok","percent":26,"resetsAt":"2026-09-21T00:00:00Z"},
///     "monthly":{"status":"ok","percent":13,"resetsAt":"2026-10-14T00:51:38Z"}}}
///   ```
///
///   只把一个数字塞进 `remaining` 会丢掉"哪个窗口先耗尽、什么时候重置"
///   —— 而这恰恰是用户要看的（滚动 5 小时窗口满了，等一会儿就恢复；
///   月度窗口满了，要等到下个月）。所以按窗口原样带给前端。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    /// 窗口标签（"滚动" / "本周" / "本月"），由后端定好，前端不猜。
    pub label: String,
    /// **剩余**百分比 0–100。
    ///
    /// 接口给的是 `percent`（**已用**百分比），这里统一换算成"剩余"
    /// 再出口 —— 界面上永远显示"还剩多少"，不需要前端各写一遍减法。
    pub remaining_percent: f64,
    /// 该窗口的重置时刻（原样字符串）。
    pub resets_at: Option<String>,
    /// 接口给的窗口状态（`"ok"` / 超额提示等），原样透传。
    pub status: Option<String>,
}

/// opencode-go 订阅额度的查询结果。与 `BtStatus` 同构：把异常状态
/// 做成数据，而不是靠 `Result::Err` 把信息丢给一句笼统的错误提示。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum QuotaStatus {
    /// 查到了。
    #[serde(rename_all = "camelCase")]
    Ok {
        /// 剩余额度数值（与 `unit` 配套）。
        ///
        /// 多窗口模式下取**最紧张的那个窗口**的剩余 —— 那是用户此刻
        /// 最该关心的数字，也是从托盘一眼能看懂的单一结论。
        remaining: f64,
        /// 额度总量；接口没给就是 `None`。
        total: Option<f64>,
        /// 已用额度；接口没给就是 `None`。
        used: Option<f64>,
        /// 单位：`"USD"` / `"CNY"` / `"credits"` / `"%"` 之类，原样透传。
        unit: String,
        /// 额度重置时间（**人类可读字符串**，不强行解析成时间戳）。
        ///
        /// ★ 这里刻意不解析成 `i64` 毫秒：接口可能给 ISO8601、可能给
        /// 相对秒数、也可能给 "next month"。解析失败就会白丢信息，
        /// 而界面只需要「什么时候恢复」这一句话。
        resets_at: Option<String>,
        /// 服务端返回的原始 JSON，折叠在详情里给排障用。
        raw: String,
        /// 各配额窗口（opencode Go 这类多窗口接口才有；单值接口为空）。
        ///
        /// `#[serde(default)]` 是为了**向后兼容**：旧版前端/旧响应没有
        /// 这个字段时，反序列化不能失败。
        #[serde(default)]
        windows: Vec<QuotaWindow>,
    },
    /// 没配 API Key。这是**未配置**，不是错误 —— 不该在界面上标红。
    NotConfigured,
    /// 鉴权失败（Key 错、过期、被吊销）。与网络错误分开，因为
    /// 「换个 Key」和「检查网络」是两件完全不同的事。
    Unauthorized { message: String },
    /// 网络层失败（超时、DNS、TLS、连不上）。
    Network { message: String },
    /// 接口返回了非 2xx，或返回体解析不出来。
    ApiError { message: String },
}

/// 一次性返回给前端的完整快照。
///
/// 两个子系统各自带 `updatedAtMs` 而不是共用一个：蓝牙 30 秒刷一次、
/// 额度 5 分钟刷一次，共用一个时间戳会让前端没法说「额度是 4 分钟前的」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightSnapshot {
    /// 蓝牙部分。
    pub bluetooth: BtStatus,
    /// 蓝牙数据的采集时刻（Unix 毫秒）。
    pub bluetooth_at_ms: u64,
    /// opencode-go 额度部分。
    pub quota: QuotaStatus,
    /// 额度数据的采集时刻（Unix 毫秒）。
    pub quota_at_ms: u64,
}

impl Default for InsightSnapshot {
    /// 冷启动时的占位值：两路都还没跑过。
    ///
    /// 蓝牙用 `Error` 而不是空 `Ok` 是**故意**的：空 `Ok` 会被前端渲染成
    /// 「无已连接设备」，而真相是「还没查」。用一个明确写着"正在初始化"的
    /// 错误态，用户第一次打开窗口看到的是「正在读取…」而不是一个断言。
    fn default() -> Self {
        Self {
            bluetooth: BtStatus::Error {
                message: "正在读取蓝牙状态…".to_string(),
            },
            bluetooth_at_ms: 0,
            quota: QuotaStatus::NotConfigured,
            quota_at_ms: 0,
        }
    }
}

/// 前后端共享的洞察状态。
///
/// 用 `Arc` 而不是让 Tauri `manage` 一个裸结构体：后台刷新任务持有它、
/// 命令处理函数也持有它，两边必须是**同一个**。Tauri 的 `State<T>`
/// 拿到的是 `&T`，只有把共享放进 `Arc` 里，spawn 出去的 tokio 任务
/// 才能克隆一份带走。
pub struct InsightState {
    /// 最新快照。`std::sync::Mutex`（不是 tokio 的）是刻意的：
    /// 临界区里只有一次结构体克隆（微秒级），没有 `.await`，
    /// 用同步锁比异步锁更省且不会引入「持锁跨 await」这类死锁。
    snapshot: std::sync::Mutex<InsightSnapshot>,
    /// 蓝牙侧缓存（含节流闸门），见 `device::DeviceCache`。
    pub(crate) devices: device::DeviceCache,
    /// 额度侧的配置（Key / 地址），见 `quota::QuotaConfig`。
    pub(crate) quota: std::sync::Mutex<quota::QuotaConfig>,
}

impl InsightState {
    /// 建一个空状态。真正的第一次刷新由 `spawn_refresh_tasks` 立刻发起。
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            snapshot: std::sync::Mutex::new(InsightSnapshot::default()),
            devices: device::DeviceCache::new(),
            quota: std::sync::Mutex::new(quota::QuotaConfig::default()),
        })
    }

    /// 读一份快照克隆。前端 `insight_current` 命令与托盘渲染都走这里。
    ///
    /// 锁中毒时不 panic 而是取回内部值 —— 与项目里 `state::lock` 同一口径：
    /// 一份可能略有偏差的展示数据，不值得让整个应用挂掉。
    pub fn snapshot(&self) -> InsightSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 只替换蓝牙那一半，保留额度与额度时间戳。
    pub(crate) fn set_bluetooth(&self, status: BtStatus) {
        let mut guard = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.bluetooth = status;
        guard.bluetooth_at_ms = now_ms();
    }

    /// 只替换额度那一半。
    pub(crate) fn set_quota(&self, status: QuotaStatus) {
        let mut guard = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.quota = status;
        guard.quota_at_ms = now_ms();
    }

    /// 读额度配置的克隆（HTTP 请求要独占配置，不能持锁发网络请求）。
    pub(crate) fn quota_config(&self) -> quota::QuotaConfig {
        self.quota
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 写额度配置。
    pub(crate) fn set_quota_config(&self, config: quota::QuotaConfig) {
        *self
            .quota
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
    }
}

/// 当前 Unix 毫秒。取不到（系统时间被拨到 1970 之前）就返回 0 ——
/// 这个值只用于界面上「几秒前更新」的展示，没有正确性要求。
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 唯一的文案出处
//
// 下面两个函数是整个子系统里**唯一**把结构化数据翻译成人话的地方。
// 前端状态栏、托盘悬浮提示、托盘右键菜单全部调它们（前端是等价的 TS 版本，
// 见 src/composables/useInsight.ts 的 deviceLine / quotaLine）。
// 允许两处实现是因为跨越了语言边界，但两边必须同步修改 —— 这一点写在
// 前端那个文件的顶部注释里。
// ---------------------------------------------------------------------------

/// 蓝牙一行文案（托盘悬浮提示 / 托盘菜单 / 状态栏气泡都用它）。
pub fn device_line(status: &BtStatus) -> String {
    match status {
        BtStatus::Ok { devices } if devices.is_empty() => "无已连接设备".to_string(),
        BtStatus::Ok { devices } => devices
            .iter()
            .map(|d| {
                let battery = match d.battery_percent {
                    Some(p) => format!("{p}%"),
                    // ★ 需求：「设备无法读取电量时电量返回 null」。
                    // 界面上不能显示 "null"，要显示人话。
                    None => "电量未知".to_string(),
                };
                format!("{}（{}·{}）", d.name, d.kind.short(), battery)
            })
            .collect::<Vec<_>>()
            .join("、"),
        BtStatus::PoweredOff => "蓝牙未开启".to_string(),
        BtStatus::NoAdapter => "无蓝牙适配器".to_string(),
        BtStatus::Error { message } => format!("蓝牙读取失败：{message}"),
    }
}

/// 额度一行文案。
pub fn quota_line(status: &QuotaStatus) -> String {
    match status {
        // 多窗口（opencode Go）：逐个窗口报**剩余**百分比。
        // 单位已经是 %，不再拼 unit（"剩余 滚动 100% · 本周 74% · 本月 87%"）。
        QuotaStatus::Ok { windows, .. } if !windows.is_empty() => {
            let parts = windows
                .iter()
                .map(|w| format!("{} {:.0}%", w.label, w.remaining_percent))
                .collect::<Vec<_>>()
                .join(" · ");
            format!("剩余 {parts}")
        }
        QuotaStatus::Ok {
            remaining,
            total,
            unit,
            ..
        } => match total {
            Some(t) if *t > 0.0 => format!("剩余 {remaining} / {t} {unit}"),
            _ => format!("剩余 {remaining} {unit}"),
        },
        QuotaStatus::NotConfigured => "未配置 API Key".to_string(),
        QuotaStatus::Unauthorized { .. } => "API Key 无效".to_string(),
        QuotaStatus::Network { .. } => "网络不可用".to_string(),
        QuotaStatus::ApiError { .. } => "接口返回异常".to_string(),
    }
}

/// 托盘悬浮提示的完整多行文本。
///
/// Windows 托盘 tooltip 对长度敏感：超过约 128 个字符（UTF-16 码元）
/// Shell_NotifyIcon 会直接失败，托盘图标连提示都不显示。设备多的时候
/// 很容易超，所以这里做**主动截断**并加省略号 —— 详情让用户去右键菜单
/// 或主窗口看（那两处没有长度限制）。
pub fn tooltip_text(snapshot: &InsightSnapshot) -> String {
    const MAX_CHARS: usize = 120;
    let mut text = format!(
        "PrisTimer\n蓝牙：{}\n额度：{}",
        device_line(&snapshot.bluetooth),
        quota_line(&snapshot.quota)
    );
    if text.chars().count() > MAX_CHARS {
        text = text.chars().take(MAX_CHARS - 1).collect::<String>();
        text.push('…');
    }
    text
}

/// 托盘右键菜单里展示蓝牙设备的条目文案（多设备各自成条）。
///
/// 与 `device_line` 的区别：这里是**一条一个设备**，而不是用「、」串起来。
/// 托盘菜单本来就是列的形态，一个设备一条比一长串更好读，也不会
/// 因为设备多而把菜单撑得没法看。
pub fn tray_device_items(status: &BtStatus) -> Vec<String> {
    match status {
        BtStatus::Ok { devices } if devices.is_empty() => {
            vec!["蓝牙：无已连接设备".to_string()]
        }
        BtStatus::Ok { devices } => devices
            .iter()
            .map(|d| {
                let battery = match d.battery_percent {
                    Some(p) => format!("{p}%"),
                    None => "电量未知".to_string(),
                };
                format!("  · {} · {} · {}", d.name, d.kind.short(), battery)
            })
            .collect(),
        BtStatus::PoweredOff => vec!["蓝牙：未开启".to_string()],
        BtStatus::NoAdapter => vec!["蓝牙：无适配器".to_string()],
        BtStatus::Error { message } => vec![format!("蓝牙：读取失败（{message}）")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, kind: DeviceKind, battery: Option<u8>) -> BtDevice {
        BtDevice {
            id: format!("id-{name}"),
            name: name.to_string(),
            kind,
            battery_percent: battery,
            battery_source: if battery.is_some() {
                BatterySource::BleBas
            } else {
                BatterySource::None
            },
            is_audio: false,
        }
    }

    /// 电量读不到时，界面文案必须是「电量未知」而不是 "null"／"None"。
    #[test]
    fn unknown_battery_renders_as_words() {
        let status = BtStatus::Ok {
            devices: vec![dev("WH-1000XM5", DeviceKind::Classic, None)],
        };
        let line = device_line(&status);
        assert!(line.contains("电量未知"), "实际文案: {line}");
        assert!(!line.contains("null"));
        assert!(!line.contains("None"));
    }

    /// 多设备必须全部罗列，且用可读的分隔符。
    #[test]
    fn multiple_devices_are_all_listed() {
        let status = BtStatus::Ok {
            devices: vec![
                dev("耳机", DeviceKind::Classic, Some(80)),
                dev("手环", DeviceKind::Ble, Some(45)),
            ],
        };
        let line = device_line(&status);
        assert!(line.contains("耳机"));
        assert!(line.contains("80%"));
        assert!(line.contains("手环"));
        assert!(line.contains("45%"));
        assert_eq!(tray_device_items(&status).len(), 2);
    }

    /// 空设备列表是合法结果，文案是「无已连接设备」，不是错误。
    #[test]
    fn empty_device_list_is_not_an_error() {
        let status = BtStatus::Ok { devices: vec![] };
        assert_eq!(device_line(&status), "无已连接设备");
        assert_eq!(tooltip_device_prefix(&status), "蓝牙：无已连接设备");
    }

    fn tooltip_device_prefix(status: &BtStatus) -> String {
        format!("蓝牙：{}", device_line(status))
    }

    /// 托盘 tooltip 必须被截断到安全长度内 —— 否则 Windows 会静默丢弃
    /// 整个 tooltip（图标连提示框都不弹），这是最难查的一类"没反应"。
    #[test]
    fn tooltip_is_truncated_for_windows_shell_limit() {
        let many = (0..40)
            .map(|i| dev(&format!("很长的设备名称{i:02}"), DeviceKind::Classic, Some(50)))
            .collect();
        let snapshot = InsightSnapshot {
            bluetooth: BtStatus::Ok { devices: many },
            bluetooth_at_ms: 0,
            quota: QuotaStatus::NotConfigured,
            quota_at_ms: 0,
        };
        let text = tooltip_text(&snapshot);
        assert!(text.chars().count() <= 120, "长度 {}", text.chars().count());
        assert!(text.ends_with('…'));
    }

    /// 每条异常都有自己的一句话，不会被压成同一个通用错误。
    #[test]
    fn error_states_are_distinguishable() {
        assert_eq!(device_line(&BtStatus::PoweredOff), "蓝牙未开启");
        assert_eq!(device_line(&BtStatus::NoAdapter), "无蓝牙适配器");
        assert!(device_line(&BtStatus::Error {
            message: "驱动异常".into()
        })
        .contains("驱动异常"));
        assert_eq!(quota_line(&QuotaStatus::NotConfigured), "未配置 API Key");
        assert_eq!(
            quota_line(&QuotaStatus::Unauthorized {
                message: "401".into()
            }),
            "API Key 无效"
        );
        assert_eq!(
            quota_line(&QuotaStatus::Network {
                message: "timeout".into()
            }),
            "网络不可用"
        );
    }

    /// 快照整份 JSON 化的字段名必须与前端 TS 接口逐字对齐。
    #[test]
    fn snapshot_serializes_to_camel_case() {
        let snapshot = InsightSnapshot {
            bluetooth: BtStatus::Ok {
                devices: vec![dev("耳机", DeviceKind::Ble, Some(66))],
            },
            bluetooth_at_ms: 1_700_000_000_000,
            quota: QuotaStatus::Ok {
                remaining: 12.5,
                total: Some(50.0),
                used: Some(37.5),
                unit: "USD".into(),
                resets_at: Some("2026-10-01".into()),
                raw: "{}".into(),
                windows: vec![],
            },
            quota_at_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["bluetooth"]["status"], "ok");
        assert_eq!(json["bluetooth"]["devices"][0]["name"], "耳机");
        assert_eq!(json["bluetooth"]["devices"][0]["kind"], "ble");
        assert_eq!(json["bluetooth"]["devices"][0]["batteryPercent"], 66);
        assert_eq!(json["bluetooth"]["devices"][0]["isAudio"], false);
        assert_eq!(json["bluetoothAtMs"], 1_700_000_000_000u64);
        assert_eq!(json["quota"]["status"], "ok");
        assert_eq!(json["quota"]["resetsAt"], "2026-10-01");

        // 未配置态只序列化 tag，不带信封 ── 前端 `status === "notConfigured"`
        // 就够，不需要再判第二个字段是否存在。
        let unconfigured = serde_json::to_value(QuotaStatus::NotConfigured).unwrap();
        assert_eq!(unconfigured["status"], "notConfigured");
        assert!(unconfigured.get("message").is_none());
    }
}
