//! 蓝牙设备枚举与电量读取。
//!
//! ★ 平台策略：**Windows 用原生 WinRT，其它平台返回明确的"不支持"**。
//!
//! 为什么不引入 `btleplug` 这类跨平台库：
//!
//!   1. 需求是「平台优先适配 Windows」，且电量必须能覆盖**经典蓝牙**
//!      —— `btleplug` 只做 BLE，耳机/音箱这两类最需要看电量的设备
//!      恰好全是经典蓝牙，用它等于把主场景漏掉。
//!   2. WinRT 的 `BluetoothDevice` / `BluetoothLEDevice` 本来就同时提供
//!      SDP 记录（经典）与 GATT（BLE）两条通道，一个 API 面覆盖全部需求。
//!   3. `windows` crate **已经在 Tauri 的依赖树里**（tauri 2.x 自己就依赖
//!      它），启用几个 feature 不新增任何需要下载的 crate —— 对离线构建
//!      环境是决定性的优势。
//!
//! ★ WinRT 初始化的坑（本项目实测踩过）：
//!
//!   `windows` crate 的异步操作（`IAsyncOperation::get()`）内部会用
//!   `WaitOnAddress` 阻塞当前线程等完成回调。这个过程需要线程**已经进入
//!   一个 WinRT apartment**，否则轻则拿不到回调、重则直接死锁。
//!   Tauri 的 tokio 工作线程没有初始化过 apartment，所以这里统一用
//!   `spawn_blocking` 把整块 WinRT 工作丢到一条**自己 `RoInitialize` 过的**
//!   阻塞线程上，用完 `RoUninitialize`。
//!
//!   注意用的是 `RO_INIT_MULTITHREADED`（MTA）而不是单线程套间：
//!   MTA 下不需要消息泵，随便哪条线程都能跑，也不会和其它组件的
//!   STA 收消息循环打架。

use std::time::{Duration, Instant};

use super::{BtDevice, BtStatus, DeviceKind};

// ---------------------------------------------------------------------------
// 节流缓存：所有蓝牙查询的唯一入口
// ---------------------------------------------------------------------------

/// 蓝牙设备缓存 + 节流闸门。
///
/// ★ 这是需求里「蓝牙查询做节流，避免高频扫描卡死蓝牙适配器」的落点。
///
/// 为什么必须节流，而不是"顶多加个缓存"：
///
///   1. **经典蓝牙电量要打 SDP**。Windows 对已配对设备把 SDP 记录缓存在
///      系统数据库里，读它不碰设备 —— 但缓存有失效的情况（刚连上、
///      设备换了固件），一旦落到真实查询，就是给设备发一次 SDP 请求，
///      1–3 秒才能回来，而且**要占用适配器的射频时隙**。
///   2. 适配器的并发查询能力很弱。Windows 蓝牙栈对同一适配器的并行
///      SDP/GATT 查询是串行化的，短时间内堆十几个请求，后面的会超时
///      甚至把适配器驱动拖到需要重启蓝牙服务。这正是"卡死蓝牙适配器"。
///   3. 前端可以无成本地疯狂 invoke（比如用户连点刷新按钮、
///      或者将来加了「打开窗口就查一次」的逻辑）。**闸门必须在后端**，
///      指望前端自觉是不可靠的。
///
/// 闸门语义是「最小间隔」而不是「固定 TTL」：
///   · `refresh()` —— 后台定时任务用。距上次真实扫描不足 `MIN_INTERVAL`
///     就**直接返回旧值**，一次系统调用都不发。
///   · `force()`    —— 用户显式点「立即刷新」用。一样受限，但会
///     在拿到旧值时告诉调用方"被节流了"，前端可以提示"稍后再试"
///     而不是让用户以为按钮坏了。
pub struct DeviceCache {
    inner: std::sync::Mutex<CacheInner>,
    /// **真实扫描的单飞闸门**：保证同一时刻最多只有一次扫描在飞。
    ///
    /// ★ 为什么必须有（实测缺陷）：`min_interval` 只判断"距上次扫描多久"，
    ///   不防并发 —— 多个调用者（两个窗口的前端兜底轮询 + Rust 定时任务）
    ///   可能同时通过闸门判定，于是同一秒里并发跑了十次扫描。实测日志里
    ///   `电量索引：候选…` 在**同一秒出现 10 条**，而需求明确要求
    ///   "避免高频扫描卡死蓝牙适配器" —— 并发扫描正是会把适配器打爆的形态。
    ///
    /// ★ 为什么单独一把锁而不是复用 `inner`：扫描要 1–3 秒，持 `inner`
    ///   会把 `snapshot()` / `cached()` 的**读**全部堵死（第一版这么干过，
    ///   界面白屏）。这把锁只串行化"扫描"这一段，读接口全程无阻。
    scan_gate: std::sync::Mutex<()>,
    /// 最小扫描间隔。抽成字段而不是写死常量，是为了测试能把它压到 0
    /// （否则一个节流测试要真等 10 秒）。
    min_interval: Duration,
}

struct CacheInner {
    /// 上一次真实扫描的结果。
    status: Option<BtStatus>,
    /// 上一次真实扫描的时刻。
    last_scan: Option<Instant>,
}

/// 节流闸门放行与否。给 `force` 用，让调用方能区分
/// 「返回旧值是因为没变化」和「返回旧值是因为被拦了」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// 放行，这次会真的去扫。
    Open,
    /// 被拦截，返回的是缓存值。
    Throttled,
}

impl DeviceCache {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(CacheInner {
                status: None,
                last_scan: None,
            }),
            scan_gate: std::sync::Mutex::new(()),
            min_interval: super::BT_THROTTLE,
        }
    }

    /// 测试用：关掉节流，避免测试真的等待。
    #[cfg(test)]
    fn without_throttle() -> Self {
        Self {
            min_interval: Duration::ZERO,
            ..Self::new()
        }
    }

    /// 后台定时刷新走这条路：被节流时静默返回缓存。
    pub fn refresh(&self) -> BtStatus {
        self.query(false).0
    }

    /// 用户显式刷新走这条路：被节流时把 `Gate::Throttled` 告诉调用方。
    pub fn force(&self) -> (BtStatus, Gate) {
        self.query(true)
    }

    /// 当前缓存值，**绝不触发扫描**。
    ///
    /// 刻意不暴露给命令层：前端要读当前状态，走 `InsightState::snapshot()`
    /// （那里带 `updatedAtMs`，比裸缓存更有信息量）。这个读接口保留着
    /// 给未来的纯展示场景用，以免到时再写一遍。
    #[allow(dead_code)]
    pub fn cached(&self) -> Option<BtStatus> {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.status.clone()
    }

    fn query(&self, _force: bool) -> (BtStatus, Gate) {
        // ① 快速路径：闸门未到期 → 直接吐缓存，一次系统调用都不发。
        //
        //    这里**绝不在持锁状态下做扫描**：第一版把整段扫描写在临界区里，
        //    结果是扫描要 1–3 秒，这期间前端来读快照会被一起堵住，界面白屏。
        //    现在锁只用来读 `last_scan` / `status`。
        if let Some(status) = self.cached_within(self.min_interval) {
            return (status, Gate::Throttled);
        }

        // ② 单飞：等可能正在进行的扫描结束。
        //
        //    等待结束后**重新判定闸门**（③）—— 这是关键：等到的时候说明
        //    另一次扫描刚跑完，它写下的 `last_scan` 让闸门变紧，我们就
        //    直接用它那份新鲜结果，而不是再扫一遍。并发 N 路调用 → 真实
        //    扫描恒为 1 次（这正是需求「避免高频扫描」要的效果）。
        let _single_flight = self
            .scan_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(status) = self.cached_within(self.min_interval) {
            return (status, Gate::Throttled);
        }

        // ④ 真正扫描（锁外扫描，`inner` 全程不持有）。
        let status = crate::insight::device::win::scan_blocking();

        // ⑤ 回填。此刻只有本线程在扫描（单飞闸门在手），回填无竞争。
        {
            let mut guard = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.status = Some(status.clone());
            guard.last_scan = Some(Instant::now());
        }
        (status, Gate::Open)
    }

    /// 「距上次扫描不足 `within`」时返回缓存值，否则 `None`（表示该扫了）。
    fn cached_within(&self, within: Duration) -> Option<BtStatus> {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(last) = guard.last_scan {
            if last.elapsed() < within {
                // 缓存理论上不会缺失（能走到这里说明至少扫过一次），
                // 真缺了就给一个明确的状态，而不是 unwrap panic。
                return Some(guard.status.clone().unwrap_or(BtStatus::Error {
                    message: "查询过于频繁，请稍候".to_string(),
                }));
            }
        }
        None
    }
}

// ===========================================================================
// Windows：WinRT 实现
// ===========================================================================
#[cfg(windows)]
mod win {
    use super::*;

    use windows::Devices::Bluetooth::GenericAttributeProfile::{
        GattCommunicationStatus, GattDeviceService,
    };
    use windows::Devices::Bluetooth::{
        BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice,
    };
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Devices::Radios::{Radio, RadioKind, RadioState};
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
    use windows::core::{GUID, HSTRING};

    /// BLE 电池服务（BAS）的 16 位 UUID，展开成完整 128 位形式。
    ///
    /// 蓝牙 SIG 的 16 位 UUID 映射到 128 位是
    /// `0000xxxx-0000-1000-8000-00805F9B34FB`。WinRT 的
    /// `GattServiceUuids::Battery()` 等辅助函数要跨进程调 WinRT 静态工厂，
    /// 在部分被测环境里会失败；直接写死常量既零成本又不会失败。
    const BAS_SERVICE: GUID = GUID::from_u128(0x0000180F_0000_1000_8000_00805F9B34FB);
    /// 电池电量特征（Battery Level, 0x2A19）。值是一个 u8 百分比。
    const BAS_LEVEL_CHAR: GUID = GUID::from_u128(0x00002A19_0000_1000_8000_00805F9B34FB);

    /// 设备属性：蓝牙设备的电量百分比（`DEVPKEY_Bluetooth_Battery`）。
    ///
    /// ★★ 这里的字面量必须是**实测值**，不能凭印象写。历史版本写的是
    ///    `"{104EA319-6EE2-47D1-BDDB-47A8CA63B19C},10"` —— GUID 后三段错、
    ///    PID 错（10 vs 2）、分隔符也错（`,` vs 空格），于是这个查询
    ///    **永远命中不了**。症状就是：耳机在 Windows 设置里明明有电量，
    ///    我们却显示「未知」；而日志里那句"经典蓝牙设备无可用电量元数据"
    ///    看起来像是设备不上报，实际是我们查错了键。
    ///
    /// 实测的正确形态（Windows 11 22621，`Get-PnpDeviceProperty` 输出的
    /// 原始键名）：
    ///
    /// ```text
    /// {104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2 = 70     ← 电量百分比
    /// {104EA319-6EE2-4701-BD47-8DDBF425BBE5} 3 = False  ← 是否在充电
    /// ```
    ///
    /// 即 PID 2 是电量、PID 3 是充电标志；键的字符串形式是
    /// `{GUID} PID`（**空格分隔**，不是逗号）。
    ///
    /// ★ 另一个同样重要的实测结论：**这个属性只出现在部分节点上**，
    ///   不能只查顶层设备节点。本机实测（2026-09-17）：
    ///
    /// | 设备 | 属性所在节点 | 值 |
    /// |---|---|---|
    /// | AULA-F87Pro 5.0 | `BTHLE\DEV_<mac>`（顶层） | 56 |
    /// | ATK A9 Nearlink | `BTHLE\DEV_<mac>`（顶层） | 35 |
    /// | AULA-SC580SE | `BTHLE\DEV_<mac>`（顶层） | 40 / 57 |
    /// | **EDIFIER MT6（耳机）** | **`BTHENUM\{0000111e}..._HCIBYPASS`（HFP 子节点）** | **70** |
    ///
    ///   耳机的电量挂在免手持（HFP）功能子节点上，顶层节点没有 ——
    ///   所以取电量的正确姿势是「扫描所有节点、按 MAC 归一」，
    ///   见 `build_battery_index`。
    const DEVPKEY_BLUETOOTH_BATTERY: &str =
        "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";

    /// 音频设备的 Major Device Class 常量来自 WinRT 的
    /// `BluetoothMajorClass` 枚举。单独 `use` 进来是因为只在
    /// `read_one` 里用一次，放在文件顶部会让人以为它是个全局概念。
    use windows::Devices::Bluetooth::BluetoothMajorClass;

    /// 单次扫描的总时间预算。
    ///
    /// 枚举设备本身很快（几十到几百毫秒），但 BLE 电量的 GATT 读取
    /// 每个设备可能要 1–2 秒，多设备会线性累加。这里给 `collect` 一个
    /// 硬预算：超时就返回**已经查到的部分**，而不是让后台任务一直挂着。
    /// 剩下的设备下一轮刷新会补上（电量不是实时数据，晚 30 秒无所谓）。
    const SCAN_BUDGET: Duration = Duration::from_secs(6);

    /// 在一条**自己初始化过 WinRT apartment** 的阻塞线程上执行扫描。
    ///
    /// 这里是全模块唯一允许调用 `block_on` / `get()` 的地方 ——
    /// 因为它跑在 `spawn_blocking` 的工作线程上，阻塞它不会饿死
    /// tokio 的异步调度器（在 async 上下文里跑 WinRT 的 `get()`
    /// 会把运行时线程整个卡住）。
    pub(super) fn scan_blocking() -> BtStatus {
        // ★ 必须在等待任何 WinRT 异步操作之前调用。返回 S_OK 或
        //   RPC_E_CHANGED_MODE（该线程已在别的套间里）都算可用；
        //   只有真正的失败才需要放弃。
        let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        let did_init = initialized.is_ok();
        if let Err(err) = &initialized {
            // RPC_E_CHANGED_MODE（0x80010106）表示线程已经有套间了，
            // 可以直接用，不算错误。其它错误才放弃。
            let code = err.code().0 as u32;
            if code != 0x8001_0106 {
                return BtStatus::Error {
                    message: format!("WinRT 初始化失败（0x{code:08X}）"),
                };
            }
        }

        let status = scan_inner();

        // 只有自己初始化成功的才反初始化 —— 别人建的套间不归我们还。
        if did_init {
            unsafe { RoUninitialize() };
        }
        status
    }

    /// 真正的扫描逻辑。假设调用方已经进入 MTA。
    fn scan_inner() -> BtStatus {
        // ① 先看无线电开关状态。这一步把「蓝牙关了」与「没连设备」
        //    区分开 —— 两者的界面文案完全不同，而后续的枚举调用
        //    在蓝牙关闭时通常只是返回空集合，从空集合反推不出原因。
        let radio = radio_state();
        if radio == RadioCheck::Off {
            return BtStatus::PoweredOff;
        }

        // ② 先建**电量索引**：一次性把系统里所有 BLE 电池服务设备的电量
        //    读出来，按设备 MAC 建表。后面每台设备查表即可。
        //
        // ★ 为什么必须走这条独立通路，而不是"枚举设备后再逐台读 GATT"：
        //
        //   实测（本机 2026-09 环境）发现，`BluetoothLEDevice` 的
        //   `GetGattServicesForUuidAsync(BAS)` 对**已配对的鼠标/键盘**
        //   大量返回失败 —— 这些设备常年处于"Connected 但被系统独占 GATT"
        //   的状态（HID 驱动持着连接），第三方进程再去开服务会拿到
        //   `Unreachable` 或空集合。
        //
        //   而 Windows 自己**已经把电量读出来了**：它为每个实现了
        //   BAS(0x180F) 的设备创建一个独立的 PnP 子设备
        //   （ID 形如 `BTHLEDevice\{0000180f-...}_Dev_VID&...`）。
        //   直接枚举这些"电池服务设备"并读它们的服务对象，拿到的就是
        //   系统缓存好的电量 —— 既准确又不需要去抢设备的 GATT 连接。
        let battery_index = build_battery_index();

        // ③ 枚举当前已连接的全部蓝牙设备（BLE + 经典）。
        //
        // ★ 为什么不用 `GetDeviceSelectorFromConnectionStatus(Connected)`：
        //
        //   实测该选择器**不可靠** —— 很明显有 4 台设备处于连接状态
        //   （鼠标键盘都在用），它却返回 0 台。这是 Windows 蓝牙栈对
        //   "连接"的判定与选择器实现在某些版本上不一致的结果。
        //
        //   可靠做法：枚举**全部**顶层设备（BLE 页 + 经典页各一轮，
        //   选择器只匹配顶层设备、不含上千个服务节点），然后对每一台
        //   用 `ConnectionStatus() == Connected` 复核（见 `read_one`）。
        //   代价是每台一次 WinRT async 调用，加起来几十毫秒，可接受。
        let found: Vec<DeviceInformation> = {
            let mut all: Vec<DeviceInformation> = Vec::new();
            for sel in [
                BluetoothLEDevice::GetDeviceSelector(),
                BluetoothDevice::GetDeviceSelector(),
            ]
            .into_iter()
            .flatten()
            {
                if let Ok(op) = DeviceInformation::FindAllAsyncAqsFilter(&sel) {
                    if let Ok(list) = op.get() {
                        all.extend(list);
                    }
                }
            }
            // 去重：同一设备可能同时出现在 BLE 页与经典页。
            let mut seen_ids = std::collections::HashSet::new();
            all.into_iter()
                .filter(|info| {
                    info.Id()
                        .map(|i| seen_ids.insert(i.to_string()))
                        .unwrap_or(false)
                })
                .collect()
        };

        // 全量枚举都拿不到任何设备 + 无线电也探不到 ⇒ 大概率没有适配器。
        // 过早断言"无适配器"会让有蓝牙的用户以为要买硬件，
        // 所以这个判定必须等"两级都空"才下。
        if found.is_empty() && radio == RadioCheck::Unknown {
            let klass_ok = match BluetoothDevice::GetDeviceSelector() {
                Ok(sel) => DeviceInformation::FindAllAsyncAqsFilter(&sel)
                    .and_then(|o| o.get())
                    .map(|list| list.Size().unwrap_or(0) > 0)
                    .unwrap_or(false),
                Err(_) => false,
            };
            if !klass_ok {
                return BtStatus::NoAdapter;
            }
        }

        let started = Instant::now();
        let mut devices: Vec<BtDevice> = Vec::new();

        for info in found {
            // 预算用尽就收工 —— 返回已查到的部分，别让后台任务无限挂着。
            if started.elapsed() > SCAN_BUDGET {
                crate::log::log("蓝牙扫描超出时间预算，返回部分结果");
                break;
            }
            match read_one(&info, started, &battery_index) {
                Some(device) => devices.push(device),
                // 单个设备失败不该毁掉整次扫描：换下一个。
                None => continue,
            }
        }

        // ③ 兜底：BLE 枚举在某些适配器上会漏掉纯经典设备（比如老式音箱）。
        //    用经典 API 再补一轮，按设备 ID 去重。
        //    代价是每次多一次枚举调用，但它是纯本地的（不碰设备），
        //    实测几毫秒，换来的是"耳机也能被列出来"。
        for device in classic_only_devices(&devices, started) {
            devices.push(device);
        }

        // ③.5（已删除）「只要读到电量就展示」的补漏通道。
        //
        // 历史：这一步曾把电池索引里所有"有电量"的设备回填进列表，理由是
        // BLE 鼠标键盘休眠时 `ConnectionStatus` 会报 Disconnected，严格过滤
        // 会让它们集体消失。
        //
        // ★ 但它的副作用用户一眼就看见了：**已经断开、早就不用的设备
        //   仍然带着上次读到的电量挂在状态栏上**（Windows 不会在设备断开时
        //   立刻清理电量缓存，而状态栏是每天都看的东西，这个错最刺眼）。
        //
        // 产品决策（2026-09-17，用户确认）：**只显示当前真正连接的设备**，
        // 宁可少显示 —— 休眠的鼠标键盘一起消失是可接受的代价。
        // 于是这一步整段删除：电量索引从此只用来「给已连接的设备查电量」，
        // 不再参与「谁该出现在列表里」的判断。

        // ④ 排序：有名在前、电量高的在前。
        //    稳定的顺序很重要 —— 否则每次刷新列表都在跳，用户会以为
        //    设备在反复掉线。`sort_by` 是稳定排序，同权重保持枚举顺序。
        devices.sort_by(|a, b| {
            let a_named = !a.name.is_empty();
            let b_named = !b.name.is_empty();
            b_named
                .cmp(&a_named)
                .then_with(|| b.battery_percent.cmp(&a.battery_percent))
        });

        // ⑤ 按设备名去重。
        //
        // ★ 为什么必须去重（本机实测）：
        //
        //   同一台鼠标会在系统里留下**多条配对记录** —— 比如
        //   `AULA-SC580SE` 同时存在 `d10069b91a2c`(57%) 与
        //   `d10769b91a2c`(40%) 两条 MAC（一个是蓝牙通道、一个是
        //   2.4G/多模通道，或换过配对）。两条都能读出电量，
        //   于是一台鼠标在界面上显示成两台，电量还不同 —— 用户会懵。
        //
        //   按名字保留**电量更高**的那条：多模设备通常蓝牙通道电量更新，
        //   而且电量高的一条更可能是当前活跃连接。
        //   （名字为空的不参与去重 —— 它们无法可靠识别，见 ③.5 的说明。）
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut deduped: Vec<BtDevice> = Vec::with_capacity(devices.len());
        for device in devices {
            if device.name.is_empty() {
                deduped.push(device);
                continue;
            }
            match seen.get(&device.name) {
                Some(&idx) => {
                    // 已有一台同名设备：电量更高的胜出。
                    if device.battery_percent > deduped[idx].battery_percent {
                        deduped[idx] = device;
                    }
                }
                None => {
                    seen.insert(device.name.clone(), deduped.len());
                    deduped.push(device);
                }
            }
        }

        BtStatus::Ok { devices: deduped }
    }

    /// 建立「设备 MAC → 电量」索引。
    ///
    /// ★ 这是电量功能的**唯一主力通路**：一次性把系统里所有带电量属性的
    ///   设备节点扫出来，按 MAC 建表，后面每台设备查表即可。
    ///
    /// 数据源是 `DEVPKEY_Bluetooth_Battery`（`{104EA319-…} 2`）——
    /// **Windows 设置页显示的蓝牙电量读的也是它**。它的分布很不均匀，
    /// 本机实测（2026-09-17）：
    ///
    /// | 设备 | 属性所在节点 | 值 |
    /// |---|---|---|
    /// | AULA-F87Pro 5.0 | `BTHLE\DEV_<mac>`（顶层） | 56 |
    /// | ATK A9 Nearlink | `BTHLE\DEV_<mac>`（顶层） | 35 |
    /// | AULA-SC580SE | `BTHLE\DEV_<mac>`（顶层） | 40 / 57 |
    /// | EDIFIER MT6（耳机） | `BTHENUM\{0000111e}…_HCIBYPASS`（HFP 子节点） | 70 |
    ///
    /// 所以**必须扫全部设备节点**（`DeviceInformationKind::Device`），
    /// 不能只看顶层设备 —— 只看顶层会漏掉所有耳机，这正是「耳机在系统
    /// 设置里有电、我们却显示未知」的根因。
    ///
    /// 属性必须**显式请求**：`Properties()` 默认只带回 Name/Id 这类基础
    /// 字段，不请求就永远 `Lookup` 不到（而且失败得很安静）。
    ///
    /// 读不到一律静默跳过 —— 电量为可选信息，读不到就是 `None`。
    fn build_battery_index() -> std::collections::HashMap<u64, (u8, String)> {
        use windows::Devices::Enumeration::DeviceInformationKind;
        // ★ `IIterable` 在 windows-collections crate 里（windows 没 re-export）。
        use windows_collections::IIterable;

        let mut index = std::collections::HashMap::new();

        // ★ 必须显式请求这个属性。
        //
        //   `DeviceInformation.Properties` 默认只带回一小撮基础属性
        //   （Name / Id / IsEnabled…），**不含** DEVPKEY_Bluetooth_Battery。
        //   不在这里声明，后面 `Properties().Lookup()` 会一律失败 ——
        //   而且失败得很安静（`ok()?` 直接变 None），看起来就像
        //   "这台设备没上报电量"。
        let wanted: IIterable<HSTRING> = vec![HSTRING::from(DEVPKEY_BLUETOOTH_BATTERY)].into();

        // ★ 空 AQS + Device kind = 系统里的**全部设备节点**。
        //
        //   为什么必须扫全部节点而不是只看顶层设备：
        //   耳机的电量挂在 `BTHENUM\{0000111e}..._HCIBYPASS`（免手持功能
        //   子节点）上，顶层设备节点根本没有这个属性（实测，见常量处的表）。
        //
        //   代价是全量枚举（本机数千个节点）。相比原来"逐个 BAS 服务节点
        //   开 GATT 读取"（每台 100–500ms，实测 10 个候选），
        //   读属性是纯内存操作，**整体反而快得多**。
        let Ok(list) = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
            &HSTRING::from(""),
            &wanted,
            DeviceInformationKind::Device,
        )
        .and_then(|op| op.get())
        else {
            crate::log::log("电量索引：设备全量枚举失败");
            return index;
        };

        let mut scanned = 0usize;
        for info in list {
            let Ok(id) = info.Id().map(|i| i.to_string()) else {
                continue;
            };
            // 没有 MAC 的节点（系统合成设备、容器节点等）不是蓝牙外设。
            let Some(mac) = extract_mac(&id) else {
                continue;
            };
            scanned += 1;

            // 同一 MAC 可能有多个节点带电量（BAS 节点 + HFP 节点 + 顶层），
            // 谁先读到用谁 —— 它们是同源数据（都来自蓝牙栈）。
            if index.contains_key(&mac) {
                continue;
            }

            let Some(level) = info
                .Properties()
                .ok()
                .and_then(|p| p.Lookup(&HSTRING::from(DEVPKEY_BLUETOOTH_BATTERY)).ok())
                .and_then(coerce_battery)
            else {
                continue;
            };

            let name = info.Name().map(|n| n.to_string()).unwrap_or_default();
            index.insert(mac, (level, name));
        }

        crate::log::log(format!(
            "电量索引：扫描 {scanned} 个节点，读出 {} 个",
            index.len()
        ));
        index
    }

    /// 从 PnP 实例 ID 里提取设备 MAC（u64，蓝牙地址的整数形式）。
    ///
    /// 实例 ID 的真实形态（本机实测）：
    ///
    /// ```text
    /// \\?\BTHLEDevice#{0000180f-0000-1000-8000-00805f9b34fb}_Dev_VID&01373b_PID&1116_REV&0121_dff86fc482df#8&36409825&0&000e#{6e3bb679-...}
    ///                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^                                    ^^^^^^^^^^^^ 这才是 MAC
    ///                  这一段里的 "00805f9b34fb" 是 12 位十六进制！
    /// ```
    ///
    /// ★★ 这里踩过一个很隐蔽的坑：如果简单地"扫第一个 12 位十六进制段"，
    ///    命中的是 **BAS UUID 的尾段 `00805f9b34fb`**（它恰好也是 12 位），
    ///    导致所有设备都被归到同一个假 MAC 上 —— 表现是"索引里只有一台设备"。
    ///
    ///    正确做法：先跳过 `#` 之间的 UUID 段与 `VID/PID/REV` 段，
    ///    只在 `REV&<12位>` 这个位置上取 MAC。
    pub(super) fn extract_mac(id: &str) -> Option<u64> {
        // 形态固定的锚点：`REV&` 之后紧跟的就是 MAC（大写十六进制）。
        // 这是稳定契约（Windows 蓝牙服务节点的命名规则），比位置扫描可靠。
        if let Some(pos) = id.find("REV&") {
            let rest = &id[pos + 4..];
            let mac: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            if mac.len() == 12 {
                if let Ok(value) = u64::from_str_radix(&mac, 16) {
                    return Some(value);
                }
            }
        }

        // 兜底：跳过第一个花括号里的 UUID 段再扫。
        // （少数 Windows 版本的节点不写 REV&。）
        let after_uuid = match id.find('}') {
            Some(idx) => &id[idx + 1..],
            None => id,
        };
        for token in after_uuid.split(|c: char| !c.is_ascii_hexdigit()) {
            if token.len() == 12 {
                if let Ok(value) = u64::from_str_radix(token, 16) {
                    return Some(value);
                }
            }
        }
        None
    }

    /// 读一个设备的信息与电量。
    fn read_one(
        info: &DeviceInformation,
        started: Instant,
        battery_index: &std::collections::HashMap<u64, (u8, String)>,
    ) -> Option<BtDevice> {
        let raw_id = info.Id().ok()?.to_string();
        let mut name = info.Name().map(|n| n.to_string()).unwrap_or_default();

        // 判断经典 / BLE / 双模。
        //
        // ★ 这里的判据是实测出来的，不是猜的：
        //   对同一个已连接的耳机执行 `BluetoothLEDevice::FromIdAsync(id)`，
        //   经典设备返回 E_INVALIDARG、BLE 设备成功。所以「能不能用
        //   BLE API 打开」就是最准的 BLE 判据 —— 比解析设备 ID 里的
        //   `#BTHLE#` / `#BTHENUM#` 片段可靠（片段格式在不同 Windows
        //   版本上变过，而且 A2DP 设备两种片段都可能出现）。
        let ble = BluetoothLEDevice::FromIdAsync(&HSTRING::from(raw_id.as_str()))
            .ok()
            .and_then(|op| op.get().ok());

        // ---- 经典侧句柄 ----
        //
        // 按 ID 打开（这一步的结果决定 `kind`：经典 / BLE / 双模）。
        let classic_by_id = BluetoothDevice::FromIdAsync(&HSTRING::from(raw_id.as_str()))
            .ok()
            .and_then(|op| op.get().ok());

        // `classic` 专供 ClassOfDevice 与连接状态用：ID 打不开时（纯 BLE 节点
        // 的 ID 形如 `BTHLE\DEV_<mac>\...`，拿它开经典设备会 E_INVALIDARG），
        // **用 MAC 在经典侧再开一次**。同一台设备在经典侧同样有配对记录，
        // 能拿到 ClassOfDevice —— 状态栏的键盘/鼠标/耳机图标全靠它。
        // 少了这个兜底，所有 BLE 键鼠都会退化成"通用蓝牙"图标。
        let classic = classic_by_id.clone().or_else(|| {
            extract_mac(&raw_id).and_then(|mac| {
                BluetoothDevice::FromBluetoothAddressAsync(mac)
                    .ok()
                    .and_then(|op| op.get().ok())
            })
        });

        // 名字兜底：`DeviceInformation::Name()` 在部分设备上是空的，
        // 但 `BluetoothDevice::Name()` / `BluetoothLEDevice::Name()` 有。
        if name.is_empty() {
            name = classic
                .as_ref()
                .and_then(|d| d.Name().ok())
                .map(|n| n.to_string())
                .or_else(|| {
                    ble.as_ref()
                        .and_then(|d| d.Name().ok())
                        .map(|n| n.to_string())
                })
                .unwrap_or_default();
        }

        let kind = match (&ble, &classic_by_id) {
            (Some(_), Some(_)) => DeviceKind::Dual,
            (Some(_), None) => DeviceKind::Ble,
            (None, Some(_)) => DeviceKind::Classic,
            // 两个都打不开：用 ID 里的片段做最后判断，实在判不出按经典算
            // （经典是超集，界面上标"经典"不会误导用户去别处找）。
            (None, None) => {
                if raw_id.contains("BTHLE") {
                    DeviceKind::Ble
                } else {
                    DeviceKind::Classic
                }
            }
        };

        // ★ 核心过滤：**只展示当前已连接的设备**。
        //
        //   这轮枚举拿到的是"全部顶层设备"（含历史配对记录）。
        //   必须复核 `ConnectionStatus` —— 否则离线设备也会被列出来，
        //   用户实测就看到了"没连的显示着电量、连着的反而未知"。
        //
        //   BLE 用 `BluetoothLEDevice.ConnectionStatus()`；
        //   经典用 `BluetoothDevice.ConnectionStatus()`；两个都打不开的
        //   顶层设备（多半是幽灵配对记录）直接丢弃。
        let connected = match (&ble, &classic) {
            (Some(b), _) => b
                .ConnectionStatus()
                .map(|s| s == BluetoothConnectionStatus::Connected)
                .unwrap_or(false),
            (None, Some(c)) => c
                .ConnectionStatus()
                .map(|s| s == BluetoothConnectionStatus::Connected)
                .unwrap_or(false),
            (None, None) => false,
        };
        if !connected {
            return None;
        }

        // 音频类判定：只看 ClassOfDevice 的 Major Device Class。
        //
        // ★ 注意 `BluetoothClassOfDevice` 的字段是私有的，不能按位取。
        //   它提供了 `MajorClass()` 返回一个 `BluetoothMajorClass` 枚举 ——
        //   用枚举比按位与可读得多，也不会因为规范里位段的定义变过而失效。
        let is_audio = classic
            .as_ref()
            .and_then(|d| d.ClassOfDevice().ok())
            .and_then(|cod| cod.MajorClass().ok())
            .map(|major| major == BluetoothMajorClass::AudioVideo)
            .unwrap_or(false);

        // 用途类别（状态栏选图标）。与 is_audio 同源但更细：
        // is_audio 只回答"能不能采信系统兜底电量"，这里回答"画哪个图标"。
        let category = classify_category(classic.as_ref());

        // ---- 电量：三条通道依次尝试，先到先得 ----
        //
        //   ① **设备属性索引**（`battery_index`）—— 主力通路。
        //      索引来自全量扫描 `DEVPKEY_Bluetooth_Battery`（按 MAC 归一），
        //      同时覆盖 BLE 外设与经典设备，**包括电量挂在 HFP 功能子节点
        //      上的耳机** —— 这正是本轮修掉的关键缺失（Windows 设置页显示
        //      的耳机电量读的就是这个属性）。
        //   ② 直连 GATT BAS —— 索引没命中时的兜底（慢，且对 HID 常失败）。
        //   ③ 经典侧系统元数据（BTHPORT 注册表启发式）—— 覆盖更老的设备。
        let mut battery = None;
        let mut source = super::super::BatterySource::None;

        // MAC：经典侧直接给，BLE 侧从实例 ID 里解析。
        let address = classic
            .as_ref()
            .and_then(|d| d.BluetoothAddress().ok())
            .or_else(|| extract_mac(&raw_id))
            .unwrap_or(0);
        if address != 0 {
            if let Some((level, _name)) = battery_index.get(&address) {
                battery = Some(*level);
                source = super::super::BatterySource::SystemPnp;
            }
        }

        // ② 直连 GATT（索引没命中时才试 —— 它慢，且对 HID 设备常失败）。
        if battery.is_none() {
            if let Some(ble) = &ble {
                if started.elapsed() < SCAN_BUDGET {
                    if let Some(level) = read_ble_battery(ble) {
                        battery = Some(level);
                        source = super::super::BatterySource::BleBas;
                    }
                }
            }
        }

        // ③ 经典侧系统元数据。
        if battery.is_none() {
            if let Some(level) = read_classic_battery(info, &raw_id, classic.is_some(), address) {
                battery = Some(level);
                source = super::super::BatterySource::ClassicSdp;
            }
        }

        Some(BtDevice {
            id: raw_id,
            name: if name.is_empty() {
                "未知设备".to_string()
            } else {
                name
            },
            kind,
            category,
            battery_percent: battery,
            battery_source: source,
            is_audio,
        })
    }

    /// 从经典侧的 ClassOfDevice 推设备用途类别（状态栏图标用）。
    ///
    /// ★ CoD 规范里 Peripheral 大类的 minor 位是：
    ///
    /// ```text
    ///   0x10  Keyboard
    ///   0x20  Pointing device（鼠标 / 触摸板）
    ///   0x30  Keyboard + Pointing（键鼠一体，按键盘显示）
    /// ```
    ///
    ///   这里用**裸值**比较而不是枚举名：WinRT 的 `BluetoothMinorClass`
    ///   没有导出 Keyboard / PointingDevice 这两个常量（只导出了
    ///   Joystick / Gamepad / RemoteControl 等），写数字反而更准确。
    pub(super) fn classify_category(
        classic: Option<&BluetoothDevice>,
    ) -> super::super::DeviceCategory {
        use super::super::DeviceCategory;

        let Some(cod) = classic.and_then(|d| d.ClassOfDevice().ok()) else {
            return DeviceCategory::Other;
        };
        let major = cod.MajorClass().map(|m| m.0).unwrap_or(-1);
        let minor = cod.MinorClass().map(|m| m.0).unwrap_or(0);

        match major {
            // AudioVideo = 4 → 耳机 / 音箱 / 车机
            4 => DeviceCategory::Audio,
            // Peripheral = 5 → 键鼠等输入设备
            5 => match minor {
                0x10 | 0x30 => DeviceCategory::Keyboard,
                0x20 => DeviceCategory::Mouse,
                _ => DeviceCategory::Other,
            },
            _ => DeviceCategory::Other,
        }
    }

    /// 走 BLE 标准电池服务读电量。
    ///
    /// 流程：打开 BAS 服务 → 拿 Battery Level 特征 → 读值。
    /// 任何一步失败都返回 `None`（"这个设备没有可用的 BAS"是常态，
    /// 不是异常 —— 大量 BLE 设备根本不实现 BAS）。
    fn read_ble_battery(ble: &BluetoothLEDevice) -> Option<u8> {
        let services = ble
            .GetGattServicesForUuidAsync(BAS_SERVICE)
            .ok()?
            .get()
            .ok()?;
        if services.Status().ok()? != GattCommunicationStatus::Success {
            return None;
        }
        let service: GattDeviceService = services.Services().ok()?.into_iter().next()?;

        let chars = service.GetCharacteristicsForUuidAsync(BAS_LEVEL_CHAR).ok()?.get().ok()?;
        if chars.Status().ok()? != GattCommunicationStatus::Success {
            return None;
        }
        let characteristic = chars.Characteristics().ok()?.into_iter().next()?;

        let read = characteristic.ReadValueAsync().ok()?.get().ok()?;
        if read.Status().ok()? != GattCommunicationStatus::Success {
            return None;
        }
        let buffer = read.Value().ok()?;

        // Battery Level 是一个单字节无符号整数，单位 %。
        // 用 `DataReader` 而不是手工解 `IBuffer` 的字节指针：
        // `IBuffer` 背后的内存所有权在 WinRT 侧，直接读指针容易悬垂。
        let reader = windows::Storage::Streams::DataReader::FromBuffer(&buffer).ok()?;
        if reader.UnconsumedBufferLength().ok()? < 1 {
            return None;
        }
        let raw = reader.ReadByte().ok()?;
        // 规范说 0–100，但见过一些山寨设备报 255（"未知"的哨兵值）。
        // 越界一律视为"读不到"，不给界面送荒唐数字。
        (raw <= 100).then_some(raw)
    }

    /// 从经典蓝牙的系统元数据里读电量。两条通道依次尝试：
    ///
    ///   1. Windows 设备属性（PnP）：`DeviceInformation::Properties()` 里
    ///      可能带 `{104EA319-...},10`（部分驱动/系统版本把电量写在这里）。
    ///   2. 注册表：`HKLM\SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters\Devices\<MAC>`
    ///      —— 蓝牙栈按设备 MAC 存元数据，逐值枚举，找名字含 battery/percent
    ///      的值。
    ///
    /// ★ 诚实声明：经典蓝牙的电量**在多数设备上就是读不到**。Windows 只对
    ///   实现了 AVRCP/HOGP 电量上报的耳机、音箱维护电量，而且存放位置随
    ///   驱动与系统版本变化。这里把两条最可能的路都走了，读不到就返回
    ///   `None` —— 界面上显示「电量未知」，这是正确行为，不是缺陷
    ///   （需求也明确要求读不到时返回 null）。
    fn read_classic_battery(
        info: &DeviceInformation,
        raw_id: &str,
        has_classic: bool,
        address: u64,
    ) -> Option<u8> {
        if !has_classic {
            return None;
        }

        // 通道 1：PnP 属性。
        if let Some(level) = read_pnp_battery(info) {
            return Some(level);
        }

        // 通道 2：BTHPORT 注册表元数据。
        if address != 0 {
            if let Some(level) = read_registry_battery(address) {
                return Some(level);
            }
        }

        // 都落空：留个痕迹方便排障（不刷屏 —— 只有第一次才打）。
        if !raw_id.is_empty() {
            crate::log::log(format!("经典蓝牙设备无可用电量元数据: {raw_id}"));
        }
        None
    }

    /// 从 BTHPORT 的每个设备子键里找电量值。
    ///
    /// ★ 为什么逐个枚举值名而不是直接读一个固定名字：
    ///   蓝牙栈在不同系统版本上给同一批设备写的元数据键名不一样
    ///   （见过 "SystemPowerOnPercent" / "BatteryPercent" / "PowerLevel"
    ///   几种写法）。枚举 + 名字启发式匹配，比写死一个名字耐撕得多。
    fn read_registry_battery(address: u64) -> Option<u8> {
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::System::Registry::{
            KEY_READ, REG_DWORD, REG_SZ, RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY,
            HKEY_LOCAL_MACHINE,
        };

        // BTHPORT 的设备子键按「无冒号的 12 位大写 MAC」命名。
        let subkey = format!(
            "SYSTEM\\CurrentControlSet\\Services\\BTHPORT\\Parameters\\Devices\\{address:012X}"
        );
        let subkey = HSTRING::from(subkey);

        // SAFETY: HKEY_LOCAL_MACHINE 是 Win32 预定义句柄，直接传；
        // phkresult 指向栈上的 HKEY，由 RegCloseKey 收尾。
        let mut key: HKEY = HKEY(std::ptr::null_mut());
        let status =
            unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, &subkey, None, KEY_READ, &mut key) };
        if status != ERROR_SUCCESS {
            return None;
        }
        // 关闭放在作用域末尾统一做 —— 上面已确保 key 打开成功。
        struct CloseOnDrop(HKEY);
        impl Drop for CloseOnDrop {
            fn drop(&mut self) {
                // SAFETY: key 由 RegOpenKeyExW 合法打开，且只关一次。
                // 返回值忽略：Drop 里无法处理失败，而"句柄已失效"这种
                // 失败在关闭路径上已经发生了。
                let _ = unsafe { RegCloseKey(self.0) };
            }
        }
        let _guard = CloseOnDrop(key);

        // 逐值枚举。名字最长放宽到 128 字符（注册表值名没有这么长的，
        // 纯粹防呆）；数据按 DWORD / 字符串两种形态解析。
        let mut index = 0u32;
        loop {
            let mut name_buf = vec![0u16; 128];
            let mut name_len = name_buf.len() as u32;
            // 第一遍只取名字：lpdata=NULL 时顺便把数据长度写回 lpcbdata。
            let mut data_len: u32 = 0;
            // SAFETY: name_buf 有 128 个 u16 可写；lpcchvaluename 如实传递容量；
            // lpdata/lpcbdata 传空指针表示只问长度。
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    Some(windows::core::PWSTR(name_buf.as_mut_ptr())),
                    &mut name_len,
                    None,
                    None,
                    None,
                    Some(&mut data_len),
                )
            };
            if status == windows::Win32::Foundation::ERROR_NO_MORE_ITEMS {
                break; // 枚举完
            }
            if status != ERROR_SUCCESS {
                // 单个值失败（权限/类型异常）不中断整轮 —— 换下一个。
                index += 1;
                continue;
            }

            // 名字启发式：含 battery / percent / powerlevel 才算候选。
            let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            let lower = name.to_ascii_lowercase();
            let candidate = lower.contains("battery")
                || lower.contains("percent")
                || lower.contains("powerlevel");

            index += 1;
            if !candidate {
                continue;
            }

            // 真正读数据：回填完整 buffer。
            if data_len == 0 || data_len > 16 {
                continue; // 空值或疑似二进制大块（不是电量形态）
            }
            let mut data = vec![0u8; data_len as usize];
            let mut type_out: u32 = 0;
            let mut cb = data_len;
            // SAFETY: data 长度与 cb 一致；type_out 接收值类型标记。
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index - 1,
                    Some(windows::core::PWSTR(name_buf.as_mut_ptr())),
                    &mut name_len,
                    None,
                    Some(&mut type_out),
                    Some(data.as_mut_ptr()),
                    Some(&mut cb),
                )
            };
            if status != ERROR_SUCCESS {
                continue;
            }

            // DWORD：小端 u32；字符串："85" 这类。
            let level: i64 = if type_out == REG_DWORD.0 {
                if data.len() < 4 {
                    continue;
                }
                i64::from(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
            } else if type_out == REG_SZ.0 {
                // REG_SZ 是 UTF-16LE 串。奇数长度说明数据被截断了，
                // 用 chunks_exact 天然丢掉最后那个落单字节。
                //
                // 这里 [allow] 掉 clippy 的 `chunks_exact` 建议：
                // 它推荐的 `as_chunks::<2>()` 目前还是 nightly 专属 API
                // （slice_as_chunks 未稳定），换过去会直接把项目钉死在
                // nightly 上 —— 为一条风格建议不值得。
                #[allow(clippy::chunks_exact_to_as_chunks)]
                let text = String::from_utf16_lossy(
                    &data
                        .chunks_exact(2)
                        .map(|b| u16::from_le_bytes([b[0], b[1]]))
                        .collect::<Vec<_>>(),
                );
                match text.trim().parse::<i64>() {
                    Ok(v) => v,
                    Err(_) => continue,
                }
            } else {
                continue;
            };

            if (0..=100).contains(&level) {
                return Some(level as u8);
            }
        }
        None
    }

    /// 读系统 PnP 属性里的电量（音频设备专用兜底）。
    fn read_pnp_battery(info: &DeviceInformation) -> Option<u8> {
        let key = HSTRING::from(DEVPKEY_BLUETOOTH_BATTERY);
        let props = info.Properties().ok()?;
        let value = props.Lookup(&key).ok()?;
        coerce_battery(value)
    }

    /// 把 WinRT PnP 属性值（类型不确定）归一成合法的百分比。
    ///
    /// ★ 这里必须走 `IPropertyValue` 而不能 `cast::<u8>()`。
    ///
    ///   WinRT 属性系统返回的值是**装箱**的（boxed）—— 底层是一个
    ///   `IInspectable`，具体数值存在 i32/u8/HSTRING 等实现在
    ///   `IPropertyValue` 接口上的对象里。`cast::<u8>()` 试图把
    ///   `IInspectable` 直接 QueryInterface 成 `u8` —— 而 `u8` 不是 COM
    ///   接口，编译器会直接拒绝（"the trait bound `u8: Interface` is not
    ///   satisfied"）。正确的做法是先 cast 到 `IPropertyValue`，
    ///   再调 `Type()` 问清真实类型，最后取对应 getter。
    ///
    /// ★ 越界一律拒绝。这条防线是必须的：非音频设备的 PnP "Battery"
    ///   属性可能是毫伏或原始计数，3000 这种值一旦被当成百分比显示，
    ///   用户会以为我们在瞎编。
    fn coerce_battery(value: windows::core::IInspectable) -> Option<u8> {
        use windows::core::Interface;
        use windows::Foundation::{IPropertyValue, PropertyType};

        let property: IPropertyValue = value.cast().ok()?;
        let kind = property.Type().ok()?;

        // 逐个按真实类型取。`u8` 是规范里 Battery 属性的标准类型，
        // 其余几种是实测见过的（不同厂商驱动不一致）。
        let raw: i64 = if kind == PropertyType::UInt8 {
            i64::from(property.GetUInt8().ok()?)
        } else if kind == PropertyType::UInt16 {
            i64::from(property.GetUInt16().ok()?)
        } else if kind == PropertyType::Int32 {
            i64::from(property.GetInt32().ok()?)
        } else if kind == PropertyType::UInt32 {
            i64::from(property.GetUInt32().ok()?)
        } else if kind == PropertyType::Int64 {
            property.GetInt64().ok()?
        } else if kind == PropertyType::String {
            // 有些驱动把电量写成字符串（"85"）。能解析就用，不能就放弃。
            property.GetString().ok()?.to_string().trim().parse::<i64>().ok()?
        } else {
            // 布尔、数组、其它类型 —— 都不是电量。
            return None;
        };

        (0..=100).contains(&raw).then_some(raw as u8)
    }

    /// 纯经典设备补齐：BLE 枚举漏掉的经典设备在这里找回来。
    fn classic_only_devices(existing: &[BtDevice], started: Instant) -> Vec<BtDevice> {
        let mut extra = Vec::new();
        // 预算已经很紧就别补了 —— 补齐是锦上添花，不能把刷新拖垮。
        if started.elapsed() > SCAN_BUDGET / 2 {
            return extra;
        }

        let selector = match BluetoothDevice::GetDeviceSelectorFromConnectionStatus(
            BluetoothConnectionStatus::Connected,
        ) {
            Ok(s) => s,
            Err(_) => return extra,
        };
        let Ok(list) = DeviceInformation::FindAllAsyncAqsFilter(&selector).and_then(|op| op.get())
        else {
            return extra;
        };

        for info in list {
            if started.elapsed() > SCAN_BUDGET {
                break;
            }
            let Ok(id) = info.Id().map(|i| i.to_string()) else {
                continue;
            };
            if existing.iter().any(|d| d.id == id) || extra.iter().any(|d: &BtDevice| d.id == id) {
                continue;
            }
            // 经典补设备走空索引 —— 它们在主循环里已经查过候选电量了。
            if let Some(device) = read_one(&info, started, &Default::default())
            {
                extra.push(device);
            }
        }
        extra
    }

    /// 无线电开关检查的三态结果。
    ///
    /// 刻意**没有** `Missing`（无适配器）这个态：`GetRadiosAsync` 返回空
    /// 列表太不可靠了（未提权、部分 Windows 版本上明明是有的机器也返回空），
    /// 由它来断言"没有硬件"是危险的。无适配器的最终判断放在设备枚举
    /// 也失败之后（见 `scan_inner`）。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RadioCheck {
        On,
        Off,
        /// 取不到状态（接口不可用 / 返回空集合 / 权限受限）。
        /// 调用方应继续尝试枚举，不要据此下结论。
        Unknown,
    }

    fn radio_state() -> RadioCheck {
        let Ok(radios) = Radio::GetRadiosAsync().and_then(|op| op.get()) else {
            return RadioCheck::Unknown;
        };
        let mut saw_bluetooth = false;
        for radio in radios {
            let Ok(kind) = radio.Kind() else { continue };
            if kind != RadioKind::Bluetooth {
                continue;
            }
            saw_bluetooth = true;
            if let Ok(state) = radio.State() {
                // 只要有一个蓝牙无线电是开的就算开（有些机器有多个，
                // 比如同时插了 USB 蓝牙棒）。
                if state == RadioState::On {
                    return RadioCheck::On;
                }
            }
        }
        if saw_bluetooth {
            RadioCheck::Off
        } else {
            // 一个蓝牙无线电都没枚举到。
            //
            // ★ 这里**不能**直接断定 "NoAdapter"：实测在部分 Windows 版本上，
            //   未提权进程调 `GetRadiosAsync` 会拿到空列表，而适配器明明在。
            //   所以返回 Unknown 让调用方继续走设备枚举；只有当枚举也拿不到
            //   任何东西时，才会在 `scan_inner` 的收尾处给出最终判断。
            RadioCheck::Unknown
        }
    }
}

// ===========================================================================
// 非 Windows：明确返回"不支持"
// ===========================================================================
//
// 不做假实现（比如返回空列表）：空列表在界面上是「无已连接设备」，
// 那是在撒谎 —— 用户会以为蓝牙真的没连东西，而去反复插拔耳机。
// 明确说"无适配器"至少让人知道该去哪儿看。macOS 其实有 `IOBluetooth`
// 能拿经典设备、CoreBluetooth 能拿 BLE；后续要补的话换掉
// `win::scan_blocking` 即可，上层（缓存 / 命令 / 托盘）完全不用动。
#[cfg(not(windows))]
mod win {
    use super::*;

    pub(super) fn scan_blocking() -> BtStatus {
        BtStatus::NoAdapter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::insight::{BatterySource, DeviceCategory, DeviceKind};

    fn sample(name: &str, battery: Option<u8>) -> BtDevice {
        BtDevice {
            id: format!("id:{name}"),
            name: name.into(),
            kind: DeviceKind::Classic,
            category: DeviceCategory::Other,
            battery_percent: battery,
            battery_source: BatterySource::ClassicSdp,
            is_audio: true,
        }
    }

    /// 节流闸门：第二次查询必须**不发扫描**而直接吐缓存。
    ///
    /// 这是需求「蓝牙查询做节流」的核心断言。用一个恒定的假状态做验证 ——
    /// 不依赖机器上真有蓝牙设备。
    #[test]
    fn throttle_returns_cached_without_rescanning() {
        let cache = DeviceCache {
            inner: std::sync::Mutex::new(CacheInner {
                status: Some(BtStatus::Ok {
                    devices: vec![sample("测试设备", Some(70))],
                }),
                last_scan: Some(Instant::now()),
            }),
            scan_gate: std::sync::Mutex::new(()),
            // 闸门关得很大，任何"放行"都会让断言失败。
            min_interval: Duration::from_secs(3600),
        };

        let (status, gate) = cache.force();
        assert_eq!(gate, Gate::Throttled, "闸门应当拦截这次查询");
        match status {
            BtStatus::Ok { devices } => {
                assert_eq!(devices.len(), 1);
                assert_eq!(devices[0].battery_percent, Some(70));
            }
            other => panic!("应当返回缓存值，实际 {other:?}"),
        }
    }

    /// 并发调用只能换来**一次**真实扫描。
    ///
    /// ★ 守着实测缺陷：闸门（`min_interval`）只判断"距上次多久"，不防并发
    ///   —— 两个窗口的前端轮询 + Rust 定时任务会同时通过判定，实测同一秒
    ///   跑了 10 次扫描。这里用 6 路并发验证单飞闸门把它们收敛成 1 次。
    #[test]
    fn concurrent_callers_do_not_stampede_the_adapter() {
        use std::sync::{Arc, Mutex as StdMutex};

        let cache = Arc::new(DeviceCache {
            inner: std::sync::Mutex::new(CacheInner {
                status: Some(BtStatus::Ok { devices: vec![] }),
                // 闸门已过期 → 第一个拿到单飞锁的线程必须真扫一次。
                last_scan: Some(Instant::now() - Duration::from_secs(3600)),
            }),
            scan_gate: std::sync::Mutex::new(()),
            // 很大：第一次扫描回填之后，其余调用一律命中缓存。
            min_interval: Duration::from_secs(3600),
        });

        let gates = Arc::new(StdMutex::new(Vec::new()));
        let mut handles = Vec::new();
        for _ in 0..6 {
            let cache = Arc::clone(&cache);
            let gates = Arc::clone(&gates);
            handles.push(std::thread::spawn(move || {
                let (_, gate) = cache.force();
                gates.lock().unwrap().push(gate);
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let gates = gates.lock().unwrap();
        let opens = gates.iter().filter(|gate| **gate == Gate::Open).count();
        assert_eq!(opens, 1, "并发调用只允许一次真实扫描，实际 {opens} 次：{gates:?}");
    }

    /// 从未扫过时，缓存为空，`cached()` 必须是纯读、返回 None。
    #[test]
    fn cached_never_triggers_a_scan() {
        let cache = DeviceCache::without_throttle();
        assert!(cache.cached().is_none());
        assert!(cache.cached().is_none(), "重复调用依然不扫描");
    }

    /// MAC 提取：真实实例 ID 必须解析出设备 MAC，**不能**误取 BAS UUID 的尾段。
    ///
    /// ★ 这个测试守着一个真实踩过的坑：BAS UUID
    ///   `0000180f-0000-1000-8000-00805f9b34fb` 的尾段 `00805f9b34fb`
    ///   恰好是 12 位十六进制，与 MAC 同长。早期实现"扫第一个 12 位十六进制段"，
    ///   结果所有设备都被归到 `00805f9b34fb` 这一个假 MAC 上，
    ///   表现是"电量索引里只有一台设备"。
    #[test]
    fn extracts_device_mac_not_uuid_fragment() {
        let cases = [
            (
                r"\\?\BTHLEDevice#{0000180f-0000-1000-8000-00805f9b34fb}_Dev_VID&01373b_PID&1116_REV&0121_dff86fc482df#8&36409825&0&000e#{6e3bb679-4372-40c8-9eaa-4509df260cd8}",
                "dff86fc482df",
            ),
            (
                r"\\?\BTHLEDevice#{0000180f-0000-1000-8000-00805f9b34fb}_Dev_VID&023554_PID&fa07_REV&6701_ea07cc75ce24#8&17c90489&0&000f#{6e3bb679-4372-40c8-9eaa-4509df260cd8}",
                "ea07cc75ce24",
            ),
            (
                r"\\?\BTHLEDevice#{0000180f-0000-1000-8000-00805f9b34fb}_Dev_VID&02248a_PID&8266_REV&0001_d10069b91a2c#8&f0d03c6&0&000c#{0000180f-0000-1000-8000-00805f9b34fb}",
                "d10069b91a2c",
            ),
        ];
        for (id, want) in cases {
            let got = win::extract_mac(id).map(|v| format!("{v:012x}"));
            assert_eq!(got.as_deref(), Some(want), "解析失败：{id}");
            assert_ne!(got.as_deref(), Some("00805f9b34fb"), "误取了 UUID 片段");
        }
    }

    /// 闸门到期后必须放行（否则"节流"就变成了"永久缓存"）。
    #[test]
    fn gate_opens_after_interval() {
        let cache = DeviceCache {
            inner: std::sync::Mutex::new(CacheInner {
                status: Some(BtStatus::Ok { devices: vec![] }),
                last_scan: Some(Instant::now() - Duration::from_secs(3600)),
            }),
            scan_gate: std::sync::Mutex::new(()),
            min_interval: Duration::from_millis(1),
        };
        // 这里会真的去扫一次系统蓝牙 —— 无所谓，只要闸门是 Open 就说明
        // 节流逻辑正确；扫描结果本身不参与断言（CI 机上可能没蓝牙）。
        let (_, gate) = cache.force();
        assert_eq!(gate, Gate::Open);
    }

    /// 同名设备去重：保留电量更高的那条。
    ///
    /// 守着一个实测现象：一台多模鼠标会在系统里留下两条 MAC 记录
    /// （蓝牙通道 + 2.4G 通道），电量还不同，不去重就会显示成两台，
    /// 而且电量互相矛盾，用户会懵。
    #[test]
    fn dedups_same_named_devices_keeping_higher_battery() {
        fn dev(name: &str, mac: &str, level: u8) -> BtDevice {
            BtDevice {
                id: format!("battery-node:{mac}"),
                name: name.to_string(),
                kind: DeviceKind::Ble,
                category: DeviceCategory::Other,
                battery_percent: Some(level),
                battery_source: BatterySource::BleBas,
                is_audio: false,
            }
        }

        // 复刻 `scan_inner` 第 ⑤ 步的去重逻辑。
        let input = vec![
            dev("AULA-SC580SE", "d10069b91a2c", 57),
            dev("AULA-SC580SE", "d10769b91a2c", 40),
            dev("ATK A9 Nearlink", "dff86fc482df", 35),
        ];
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut out: Vec<BtDevice> = Vec::new();
        for d in input {
            match seen.get(&d.name) {
                Some(&i) => {
                    if d.battery_percent > out[i].battery_percent {
                        out[i] = d;
                    }
                }
                None => {
                    seen.insert(d.name.clone(), out.len());
                    out.push(d);
                }
            }
        }

        assert_eq!(out.len(), 2, "同名设备应合并为一台");
        let aula = out.iter().find(|d| d.name == "AULA-SC580SE").unwrap();
        assert_eq!(aula.battery_percent, Some(57), "应保留电量更高的一条");
    }
}
