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
// ★ 快照的写入口只有两处：`applyBluetooth`（蓝牙，直接覆盖）与 `ingestQuota`
//   （额度，走 `planQuotaIngest` 的"软失败保留旧值"规则）。任何新链路都必须
//   汇到这两处，不要再直接 `snapshot.value = ...`。
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

/**
 * 「保留旧值」的**上限**（2026-09-17 第 30 轮）。
 *
 * 周期查询偶发失败时我们保留上一次的额度值（见 `planQuotaIngest`），
 * 但保留不是无限的：后端 5 分钟一刷，连续 6 次都问不到说明网络是真的断了，
 * 这时候还端着一小时前的数字说"本月 87%"就是误导 —— 比红色错误更糟。
 * 超过这个时长就如实落地失败状态，把"问不到"摆在明面上。
 */
const QUOTA_HOLD_MAX_MS = 30 * 60_000;

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
// 额度落地规则（唯一的决策点，导出以便单测锁定语义）
// ---------------------------------------------------------------------------

/** 「最近一次额度查询没问到」的软状态。 */
export interface QuotaStale {
  /** 失败发生的时刻（Unix 毫秒）。 */
  atMs: number;
  /** 人话原因（`quotaLine` 的输出，如"网络不可用"）。 */
  message: string;
}

/** `planQuotaIngest` 的结论：落地这次结果，还是保留旧值只标陈旧。 */
export type QuotaIngestPlan = "apply" | "hold";

/**
 * 一次额度查询结果该不该覆盖快照。
 *
 * ★ 问题（用户 2026-09-17 拍板要改的行为）：
 *   窗口里那个数值是**上一次成功**的结果。周期性查询（Rust 后台 5 分钟一次、
 *   前端过期兜底）偶发超时 —— 实测日志里就有 `WinHTTP 12002` —— 旧实现
 *   无条件覆盖，一次超时就把 5 分钟前刚拿到的 87% 糊成红色"网络不可用"。
 *   用户看到的是"数据没了"，而事实只是"这一次没问到"。
 *
 * ★ 所以：**保留旧值 + 标记陈旧**，把"问不到"和"没有值"分开表达。但三种
 *   情况必须如实落地，不能藏：
 *
 *   · `user` 显式动作（点「立即刷新」、保存配置）—— 用户刚动了手，必须
 *     看到结果，否则就是"点了没反应"；
 *   · `notConfigured` —— 这是配置状态而非网络抖动，藏着会让用户以为已经配好了；
 *   · `unauthorized` —— 401 是凭据被拒，属于可行动的硬错误，不该被降级成暗色。
 *
 * ★ 手里没有可保留的好值时（首次查询就失败）也只能如实落地 ——
 *   "陈旧"标记的前提是**有旧值**。跨重启不保留（旧值不落盘）：拿一个
 *   昨天的数字当"本月额度"比红色错误更误导。
 *
 * 纯函数，无副作用 —— 单测直接喂参数就能覆盖全部分支。
 */
export function planQuotaIngest(
  next: QuotaStatus,
  held: { quota: QuotaStatus; atMs: number },
  opts: { user?: boolean; now: number },
): QuotaIngestPlan {
  if (next.status === "ok") return "apply";
  if (opts.user) return "apply";
  if (next.status === "notConfigured" || next.status === "unauthorized") return "apply";
  if (held.quota.status !== "ok") return "apply";
  if (opts.now - held.atMs >= QUOTA_HOLD_MAX_MS) return "apply";
  return "hold";
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

/**
 * 设备名缩写 —— 状态栏与悬浮窗都要用（一行要塞好几台设备）。
 *
 * 规则（按顺序）：
 *   1. 有连字符取后段 —— `AULA-SC580SE` → `SC580SE`
 *   2. 去掉开头的纯品牌词 —— `EDIFIER MT6` → `MT6`、`ATK A9 Nearlink` → `A9 Nearlink`
 *   3. 截到第一个空格 —— `A9 Nearlink` → `A9`、`F87Pro 5.0` → `F87Pro`
 *
 * 全名始终保留在 DOM 的 `title` 里（悬停可见），所以缩错了也不会丢信息。
 */
export function shortName(name: string): string {
  if (!name) return "未知设备";
  let s = name.trim();

  const dash = s.lastIndexOf("-");
  if (dash > 0 && dash < s.length - 1) s = s.slice(dash + 1);

  const brand = s.match(/^([A-Z][A-Z0-9]{2,})\s+(.+)$/);
  if (brand) s = brand[2];

  const space = s.indexOf(" ");
  if (space > 0) s = s.slice(0, space);

  return s.length > 14 ? `${s.slice(0, 13)}…` : s;
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
      // ★ 不要写成"API Key 无效"（2026-09-17 修正）。
      //   实测：同一把 Key 在 /zen/v1/chat/completions 上能通过鉴权
      //   （返回 400 Model is unavailable 而非 401），说明**凭据本身有效**；
      //   被拒的是额度那个端点。断言"Key 无效"会把用户引向错误的排查方向。
      //   这里只报事实（401，服务端拒绝凭据），排查方向交给用户判断。
      return "额度不可用（401）";
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

/**
 * 「最近一次额度查询没问到」的软状态 —— 此时 `snapshot.quota` 里是**上一次
 * 成功拿到的好值**，而不是失败本身（决策规则见 `planQuotaIngest`）。
 *
 * 展示层用它把"数据是旧的"这件事说清楚：状态栏加一枚"陈旧"小标、迷你窗把
 * 额度芯片降饱和、悬浮窗补一行"刷新失败 · 显示 N 分钟前的数据"。
 * 它的存在**不动** `quotaAtMs` —— 那个时间戳始终是"这个值是什么时候拿到的"，
 * 保留旧值时它就该继续变老，`ageLabel` 才会诚实地说"12 分钟前"。
 */
const quotaStale = ref<QuotaStale | null>(null);

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
      // ★ 不整个 `snapshot.value = event.payload`（2026-09-17 第 30 轮）：
      //   后端的周期任务失败时**也会**推一份带失败额度状态的快照
      //   （`spawn_quota_task` 无论成败都 `publish`），整个赋值就等于
      //   把"这一次没问到"当成"没有数据"糊上去。拆开各自落地。
      applySnapshot(event.payload);
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
    if (current) applySnapshot(current);
  } catch {
    /* 保持空快照，兜底轮询会补 */
  }
}

// ---------------------------------------------------------------------------
// 动作
// ---------------------------------------------------------------------------

/**
 * 把一份**后端推来的完整快照**拆开落地。
 *
 * 蓝牙直接覆盖（它没有"闪红"问题：没设备就是没设备，两种表达等价）；
 * 额度走 `ingestQuota` 的软失败规则。两个时间戳都取自载荷 —— 那是数据
 * 真正被取到的时刻，不是前端的接收时刻，本机时钟偏一点也不会漂。
 */
function applySnapshot(next: InsightSnapshot): void {
  snapshot.value = {
    ...snapshot.value,
    bluetooth: next.bluetooth,
    bluetoothAtMs: next.bluetoothAtMs || Date.now(),
  };
  ingestQuota(next.quota);
}

/**
 * 额度结果的**唯一写入口**。决策交给 `planQuotaIngest`，这里只执行。
 */
function ingestQuota(status: QuotaStatus, opts: { user?: boolean } = {}): void {
  const now = Date.now();
  const plan = planQuotaIngest(
    status,
    { quota: snapshot.value.quota, atMs: snapshot.value.quotaAtMs },
    { user: opts.user, now },
  );

  if (plan === "apply") {
    quotaStale.value = null;
    snapshot.value = { ...snapshot.value, quota: status, quotaAtMs: now };
    return;
  }
  // "hold"：`quota` 与 `quotaAtMs` 都原地不动（那个值确实是那个时刻拿到的），
  // 只记下"这一次没问到"，交给展示层弱化表达。
  quotaStale.value = { atMs: now, message: quotaLine(status) };
}

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

/** 用户点「立即刷新额度」。用户显式动作 → 失败如实落地，不做软处理。 */
async function refreshQuota(): Promise<void> {
  try {
    const status = await insightApi.quotaQuery();
    applyQuota(status, { user: true });
  } catch {
    /* 失败留给下一次事件/轮询 */
  }
}

/** 直接把最新额度状态写进快照（配置保存等场景）。
 *
 *  ★ 默认就是 `user` 语义 —— 这个函数的调用方只有"用户刚保存完配置"这一处，
 *    它的结果必须如实显示（用户等着看"配好了没"）。周期性链路一律走
 *    `applySnapshot` / `ingestQuota`，不经过这里。 */
function applyQuota(status: QuotaStatus, opts: { user?: boolean } = { user: true }): void {
  ingestQuota(status, opts);
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
  /** 最近一次额度查询失败的软状态（此时 `snapshot.quota` 仍是上一次的好值）。 */
  quotaStale: Ref<QuotaStale | null>;
  /** 托盘是否已就绪。 */
  trayReady: Ref<boolean>;
  /** 最近一次「立即刷新」是否被节流。 */
  lastBtThrottled: Ref<boolean>;
  refreshBluetooth: () => Promise<boolean>;
  refreshQuota: () => Promise<void>;
  /** 保存额度配置后落地返回的最新状态（用户语义：失败如实显示）。 */
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
    quotaStale,
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