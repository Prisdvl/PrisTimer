// ---------------------------------------------------------------------------
// useInsight：系统信息（蓝牙设备 + opencode-go 额度）的模块级单例。
//
// ★ 与项目里 useTheme / usePomodoro / useTags 同一模式：状态在模块作用域，
//   所有组件共享同一份数据与**同一条**事件订阅 —— 不会因为组件挂载多次
//   而重复 subscribe、重复 invoke。
//
// ★ 数据流（彻底贯彻「后端定时刷新，前端只读缓存」）：
//
//   Rust 后台任务（蓝牙 30s / 额度 5min）→ insight:update 事件 → 这里更新快照。
//   前端**只**承担两件事：
//     1. 订阅事件 + 挂载时拉一次初值（`insightApi.current()`）；
//     2. 一个低速的"过期兜底"轮询 —— 万一事件丢了（极端情况），
//        数据也不会永远停在启动那一刻。
//   前端**不**承担周期扫描 —— 那会变成「每个窗口各自刷」的多实例竞态。
//
// ★ 文案出处：`deviceLine` / `quotaLine` 是本文件里唯一的文案工厂，
//   与 Rust 侧 `insight::tooltip_text` 的分支逐条对齐（两边必须同步改，
//   这是跨语言共享文案的固定成本）。状态栏 / 悬浮窗 / 未来任何展示
//   都从这里取，不许各自拼字符串。
// ---------------------------------------------------------------------------

import { ref, type Ref } from "vue";
import { listen } from "@tauri-apps/api/event";
import { insightApi, type BtDevice, type InsightSnapshot, type QuotaStatus, type BtStatus } from "../api";

// ---------------------------------------------------------------------------
// 刷新周期（与 Rust 侧同步，信任但校验）
// ---------------------------------------------------------------------------

/** 蓝牙数据的最大"可接受陈旧度"。后端 30s 刷一次，这里 35s 兜底。 */
const BT_STALE_MS = 35_000;
/** 额度数据的最大"可接受陈旧度"。后端 5min 刷一次，这里 5.5min 兜底。 */
const QUOTA_STALE_MS = 5.5 * 60_000;
/** 兜底轮询检查间隔。1 秒一次做"过期判定"足够了。 */
const POLL_INTERVAL_MS = 1_000;

// ---------------------------------------------------------------------------
// 类型守卫
// ---------------------------------------------------------------------------

export function isBtOk(status: BtStatus): status is Extract<BtStatus, { status: "ok" }> {
  return status.status === "ok";
}

export function isQuotaOk(status: QuotaStatus): status is Extract<QuotaStatus, { status: "ok" }> {
  return status.status === "ok";
}

// ---------------------------------------------------------------------------
// 文案工厂（唯一出处 —— 与 Rust 侧对齐）
// ---------------------------------------------------------------------------

/** 设备类型短标签。与 Rust 侧 `DeviceKind::short()` 对齐。 */
export function kindLabel(kind: BtDevice["kind"]): string {
  switch (kind) {
    case "classic":
      return "经典";
    case "ble":
      return "BLE";
    case "dual":
      return "双模";
  }
}

/** 电量文案。null → "未知"，绝不显示 "null"。 */
export function batteryText(percent: number | null): string {
  return percent === null ? "电量未知" : `${percent}%`;
}

/** 蓝牙整体一句话。分支与 Rust 侧 `device_line` 对齐。 */
export function deviceLine(status: BtStatus): string {
  switch (status.status) {
    case "ok":
      if (status.devices.length === 0) return "无已连接设备";
      return status.devices
        .map((d) => `${d.name}（${kindLabel(d.kind)}·${batteryText(d.batteryPercent)}）`)
        .join("、");
    case "poweredOff":
      return "蓝牙未开启";
    case "noAdapter":
      return "无蓝牙适配器";
    case "error":
      return `蓝牙读取失败：${status.message}`;
  }
}

/** 额度一句话。分支与 Rust 侧 `quota_line` 对齐。 */
export function quotaLine(status: QuotaStatus): string {
  switch (status.status) {
    case "ok": {
      const { remaining, total, unit, windows } = status;
      // 多窗口（opencode Go）：逐窗口报**剩余**百分比，与 Rust 侧同款文案。
      if (windows && windows.length > 0) {
        return `剩余 ${windows.map((w) => `${w.label} ${Math.round(w.remainingPercent)}%`).join(" · ")}`;
      }
      return total !== null && total > 0 ? `剩余 ${remaining} / ${total} ${unit}` : `剩余 ${remaining} ${unit}`;
    }
    case "notConfigured":
      return "未配置 API Key";
    case "unauthorized":
      return "API Key 无效";
    case "network":
      return "网络不可用";
    case "apiError": {
      // ★ 错误细节要露出来：早期版本只显示"接口返回异常"，
      //   用户对着 404 / 401 完全不知道怎么办。带上一句简短原因。
      const brief = status.message.slice(0, 22);
      return `接口异常：${brief}${status.message.length > 22 ? "…" : ""}`;
    }
  }
}

/** 「多少秒/分钟前更新」的人话。 */
export function ageLabel(ms: number): string {
  const seconds = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (seconds < 10) return "刚刚";
  if (seconds < 60) return `${seconds} 秒前`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟前`;
  return `${Math.round(minutes / 60)} 小时前`;
}

// ---------------------------------------------------------------------------
// 模块级单例状态
// ---------------------------------------------------------------------------

/** 与 Rust 侧 `InsightSnapshot::default()` 对齐的空快照。 */
function emptySnapshot(): InsightSnapshot {
  return {
    bluetooth: { status: "error", message: "正在读取蓝牙状态…" },
    bluetoothAtMs: 0,
    quota: { status: "notConfigured" },
    quotaAtMs: 0,
  };
}

const snapshot = ref<InsightSnapshot>(emptySnapshot());

/** 托盘是否已就绪（Rust 建好托盘后发 `insight:tray-ready`）。
 *  只有就绪后「显示信息浮窗」按钮才有意义。 */
const trayReady = ref(false);

/** 「立即刷新蓝牙」的节流回执 —— 前端提示用。 */
const lastBtThrottled = ref(false);

let started = false;
let pollTimer: number | null = null;

async function subscribe(): Promise<void> {
  try {
    // 单例只订阅一次、且存活与应用同长，不需要保留 unlisten 句柄
    // （组件各自挂载/卸载互不影响 —— 状态在模块作用域，不随组件销毁）。
    await listen<InsightSnapshot>("insight:update", (event) => {
      snapshot.value = event.payload;
      // ★ `insight:update` 能到达，说明 Rust 的托盘与刷新任务都已就绪
      //   （它们在同一处装配）。用它给 trayReady 兜底 ——
      //   「托盘就绪」事件可能在页面加载完成之前就广播了，
      //   主窗口会错过那一次，只有靠这个等价信号补上。
      trayReady.value = true;
    });
  } catch {
    // 事件订阅失败（非常早期）不致命：兜底轮询会接管数据新鲜度。
  }
  try {
    // 托盘就绪事件：Rust 建好托盘后广播。载荷为空。
    await listen("insight:tray-ready", () => {
      trayReady.value = true;
    });
  } catch {
    /* 同上 */
  }
}

/** 过期兜底轮询。只在"数据明显比周期旧"时补一次拉取。 */
function startPolling(): void {
  if (pollTimer !== null) return;
  pollTimer = window.setInterval(() => {
    const now = Date.now();
    const s = snapshot.value;
    if (now - s.bluetoothAtMs > BT_STALE_MS) {
      insightApi.btDevices().catch(() => {});
    }
    if (now - s.quotaAtMs > QUOTA_STALE_MS) {
      insightApi.quotaQuery().catch(() => {});
    }
  }, POLL_INTERVAL_MS);
}

async function init(): Promise<void> {
  if (started) return;
  started = true;

  await subscribe();

  // ★ 只有主窗口做「过期兜底轮询」。
  //
  //   悬浮信息窗（`index.html#overlay`）跑的是同一份 bundle 的
  //   第二个 webview，模块级的 `started` 标志**不跨窗口共享** ——
  //   不加这个判断，两个窗口会各自每秒轮询一次，把蓝牙扫描请求
  //   整体翻倍（实测日志里同一秒出现 10 条扫描记录的直接来源之一）。
  //   浮窗只需要订阅 `insight:update` 拿数据，不需要自己催。
  const isOverlayWindow = window.location.hash === "#overlay";
  if (!isOverlayWindow) {
    startPolling();
  }

  // 拉初值。事件是推送式的，订阅之前的那一帧不会补发，
  // 必须主动拉一次 —— 与 timer:update 的处理完全同构。
  try {
    const current = await insightApi.current();
    if (current) snapshot.value = current;
  } catch {
    /* 保持空快照，兜底轮询会补 */
  }
}

// ---------------------------------------------------------------------------
// 动作
// ---------------------------------------------------------------------------

/** 用户点「立即刷新」：带节流回执。 */
async function refreshBluetooth(): Promise<boolean> {
  try {
    const outcome = await insightApi.btRefresh();
    if (outcome.bluetooth) {
      // 命令附带返回了最新状态，直接落地（事件推送通常也马上到，双写幂等）。
      applyBluetooth(outcome.bluetooth);
    }
    lastBtThrottled.value = outcome.throttled;
    if (outcome.throttled) {
      // 节流提示只闪 2.5 秒就消失，不常驻干扰。
      window.setTimeout(() => {
        lastBtThrottled.value = false;
      }, 2500);
    }
    return outcome.throttled;
  } catch {
    return false;
  }
}

/** 用户点「立即刷新额度」。 */
async function refreshQuota(): Promise<void> {
  try {
    const status = await insightApi.quotaQuery();
    applyQuota(status);
  } catch {
    /* 失败留给下一次事件/轮询 */
  }
}

/** 直接把最新额度状态写进快照（配置保存等场景）。 */
function applyQuota(status: QuotaStatus): void {
  snapshot.value = { ...snapshot.value, quota: status, quotaAtMs: Date.now() };
}

/** 直接把最新蓝牙状态写进快照。 */
function applyBluetooth(status: BtStatus): void {
  snapshot.value = { ...snapshot.value, bluetooth: status, bluetoothAtMs: Date.now() };
}

export interface UseInsight {
  /** 最新快照。引用是模块级单例，唯一写入口在本文件内部。
   *  （返回裸 ref 而不是 `readonly()`：Vue 的 DeepReadonly 会把
   *   `devices: BtDevice[]` 变成 `readonly [...]`，与接口的数组类型
   *   互相不兼容，类型上反而更别扭。） */
  snapshot: Ref<InsightSnapshot>;
  /** 托盘是否已就绪。 */
  trayReady: Ref<boolean>;
  /** 最近一次「立即刷新」是否被节流。 */
  lastBtThrottled: Ref<boolean>;
  refreshBluetooth: () => Promise<boolean>;
  refreshQuota: () => Promise<void>;
  /** 保存额度配置后落地返回的最新状态。 */
  applyQuota: (status: QuotaStatus) => void;
  applyBluetooth: (status: BtStatus) => void;
  deviceLine: (status: BtStatus) => string;
  quotaLine: (status: QuotaStatus) => string;
}

/** 入口。组件在 setup 里调一次即可拿到全部状态（幂等初始化）。 */
export function useInsight(): UseInsight {
  void init();
  return {
    snapshot,
    trayReady,
    lastBtThrottled,
    refreshBluetooth,
    refreshQuota,
    applyQuota,
    applyBluetooth,
    deviceLine,
    quotaLine,
  };
}