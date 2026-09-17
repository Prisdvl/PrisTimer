import { invoke } from "@tauri-apps/api/core";

// ---------------------------------------------------------------------------
// 与 Rust 侧一一对应的类型。
//
// 字段名必须与 Rust 结构体上的 `#[serde(rename_all = "camelCase")]` 完全一致：
// Rust 的 `elapsed_ms` ↔ 前端的 `elapsedMs`，`day` 这类单词字段则保持原样。
// 写错一个字母，运行时只会得到 `undefined`，不会报错 —— 这是 Tauri 项目里
// 最常见的静默失败。
// ---------------------------------------------------------------------------

export type TimerState = "idle" | "running" | "paused" | "finished";

export interface TimerSnapshot {
  state: TimerState;
  elapsedMs: number;
  remainingMs: number | null;
  limitMs: number | null;
}

export type SessionKind = "stopwatch" | "countdown";

export interface DailyStat {
  /** 本地时区的 `YYYY-MM-DD`。 */
  day: string;
  totalMs: number;
  sessionCount: number;
  completedCount: number;
}

export interface StatsSummary {
  totalMs: number;
  sessionCount: number;
}

/** 按标签（科目）聚合的专注时长。tag 为空串表示未标注。 */
export interface TagTotal {
  tag: string;
  totalMs: number;
  sessionCount: number;
}

/** 某一天、某一个科目的专注总时长。 */
export interface TagDayTotal {
  /** 本地时区的 `YYYY-MM-DD`。 */
  day: string;
  /** 空串表示未标注。 */
  tag: string;
  totalMs: number;
}

/** 某个本地小时（0–23）的专注总时长，按会话开始时刻归属。 */
export interface HourTotal {
  hour: number;
  totalMs: number;
  sessionCount: number;
}

/** 每日复习目标：科目 → 目标分钟数（1–1440）。 */
export type TagGoals = Record<string, number>;

export interface Recovered {
  id: number;
  kind: SessionKind;
  startedAt: number;
  elapsedMs: number;
  limitMs: number | null;
  ageMs: number;
}

export type PomodoroPhase = "focus" | "short_break" | "long_break";

export interface PomodoroStatus {
  enabled: boolean;
  phase: PomodoroPhase;
  completedFocus: number;
}

/** 番茄循环参数。毫秒与 Rust 同口径；分钟数由 UI 层换算。 */
export interface PomodoroConfig {
  focusMs: number;
  shortBreakMs: number;
  longBreakMs: number;
  /** 连续几个专注后进长休。 */
  focusBeforeLong: number;
}

export const timerApi = {
  start: () => invoke<void>("timer_start"),
  pause: () => invoke<void>("timer_pause"),
  reset: () => invoke<void>("timer_reset"),
  setLimit: (limitMs: number | null) => invoke<void>("timer_set_limit", { limitMs }),

  /** 启动时是否恢复出一条未完成的会话。 */
  recovered: () => invoke<Recovered | null>("recovered_session"),

  /** 当前计时快照。订阅事件之前推送的快照不会补发，挂载时需主动拉一次。 */
  current: () => invoke<TimerSnapshot | null>("timer_current"),

  /** 区间按日聚合；`fromDay` / `toDay` 为本地日期 `YYYY-MM-DD`，含两端。 */
  dailyStats: (fromDay: string, toDay: string) =>
    invoke<DailyStat[]>("stats_daily", { fromDay, toDay }),

  summary: () => invoke<StatsSummary>("stats_summary"),

  /** 按科目聚合的专注时长（无标签为空串）；日期范围含两端。 */
  statsTags: (fromDay: string, toDay: string) =>
    invoke<TagTotal[]>("stats_tags", { fromDay, toDay }),

  /** 按「本地日 × 科目」聚合的专注时长；日期范围含两端。 */
  statsTagDaily: (fromDay: string, toDay: string) =>
    invoke<TagDayTotal[]>("stats_tag_daily", { fromDay, toDay }),

  /** 按会话开始时刻的本地小时（0–23）聚合；日期范围含两端。无记录的小时不返回。 */
  statsHourly: (fromDay: string, toDay: string) =>
    invoke<HourTotal[]>("stats_hourly", { fromDay, toDay }),

  /** 设置下一条会话的科目；空串/空白视为清除，返回规范化后的值。 */
  tagSet: (tag: string | null) => invoke<string | null>("tag_set", { tag }),

  /** 当前待用科目（挂载时回显用）。 */
  tagCurrent: () => invoke<string | null>("tag_current"),

  /** 读写每日复习目标（科目 → 分钟数）。Rust 侧校验：科目 ≤20 字、分钟 1–1440、最多 8 个。 */
  tagGoalsGet: () => invoke<TagGoals>("tag_goals_get"),
  tagGoalsSet: (goals: TagGoals) => invoke<TagGoals>("tag_goals_set", { goals }),

  /**
   * 重命名（合并/清除）历史会话标签：所有 tag=from 的会话改为 to。
   * to 传 null 表示清为未标注；返回改写条数。当前标签与每日目标联动迁移。
   */
  tagRename: (from: string, to: string | null) =>
    invoke<number>("tag_rename", { from, to }),

  /** 导出全部已结算会话为 CSV 到下载目录，返回文件完整路径。 */
  exportCsv: (filename: string) => invoke<string>("export_csv", { filename }),

  /**
   * 导出指定区间的「周报」CSV（区间汇总 + 科目汇总 + 每日明细三段式）到
   * 下载目录，返回文件完整路径。日期为本地 `YYYY-MM-DD`，含两端。
   */
  exportReportCsv: (filename: string, fromDay: string, toDay: string) =>
    invoke<string>("export_report_csv", { filename, fromDay, toDay }),

  /** 开关番茄模式。 */
  pomodoroSet: (enabled: boolean) =>
    invoke<PomodoroStatus>("pomodoro_set", { enabled }),

  /** 当前番茄状态。与计时快照同理：订阅前的事件不补发，挂载时拉一次。 */
  pomodoroCurrent: () => invoke<PomodoroStatus>("pomodoro_current"),

  /** 读写番茄配置（Rust 侧负责校验与落库，越界会被拒绝）。 */
  pomodoroConfigGet: () => invoke<PomodoroConfig>("pomodoro_config_get"),
  pomodoroConfigSet: (config: PomodoroConfig) =>
    invoke<PomodoroStatus>("pomodoro_config_set", { config }),
};

/** 常规形态的几何：客户区尺寸 + 外框左上角（与 Rust 侧 set_size / set_position 同坐标系）。 */
export interface NormalGeom {
  w: number;
  h: number;
  x: number;
  y: number;
}

export interface MiniPos {
  x: number;
  y: number;
}

export interface WinState {
  normal: NormalGeom | null;
  maximized: boolean;
  miniMode: boolean;
  mini: MiniPos | null;
}

/**
 * 窗口几何持久化。
 *
 * 状态存在 Rust 侧（应用数据目录下的 `window.json`），**启动时的恢复也由 Rust
 * 完成** —— 窗口必须以记忆中的几何直接出现，而不是先露出默认位置再跳过去。
 * 前端只负责「变动时上报」：用户拖动/缩放后把新几何写回去。
 *
 * 全部字段可选：只传自己知道的那部分，Rust 侧做合并。
 */
export const windowApi = {
  get: () => invoke<WinState>("win_state_get"),
  save: (patch: {
    normal?: NormalGeom;
    maximized?: boolean;
    miniMode?: boolean;
    mini?: MiniPos;
  }) => invoke<WinState>("win_state_save", patch),
  /** 原生窗口几何动画：Rust 侧 SetWindowPos 分帧插值（easeOutCubic），
   *  一次 invoke 完成整段动画。尺寸/位置均为**物理**像素；
   *  尺寸语义与 set_size 一致（客户区）。失败时调用方退回 JS 插值。 */
  animateTo: (to: { x: number; y: number; w: number; h: number; durationMs?: number }) =>
    invoke<null>("animate_window_to", {
      x: to.x,
      y: to.y,
      w: to.w,
      h: to.h,
      durationMs: to.durationMs ?? 320,
    }),
  /** 迷你形态窗口属性开关：一次 IPC 完成 resizable/min-size/always-on-top/shadow
   *  四项设置（逐项 IPC 实测 ~21ms/次，串行 4 次在动画关键路径上白占 ~80ms）。 */
  setMiniShell: (mini: boolean) => invoke<null>("set_mini_shell", { mini }),
};

// ---------------------------------------------------------------------------
// 系统信息（蓝牙设备 + opencode-go 额度）
//
// ★ 与 Rust 侧 insight::* 一一对应。新增字段时两端必须同步 ——
//   这是「跨语言边界共享数据结构」的固定成本，只能靠约定。
// ---------------------------------------------------------------------------

/** 蓝牙设备类型。与 Rust 侧 `DeviceKind` 的序列化值对齐。 */
export type BtKind = "classic" | "ble" | "dual";

/** 电量来源。与 Rust 侧 `BatterySource` 对齐。 */
export type BatterySource = "bleBas" | "classicSdp" | "systemPnp" | "none";

/**
 * 设备用途类别（驱动状态栏的类型图标）。与 Rust 侧 `DeviceCategory` 对齐。
 *
 * ★ 与 `BtKind` 是两个维度：`kind` 说"怎么连的"（经典/BLE/双模），
 *   `category` 说"它是干什么的"（耳机/键盘/鼠标）。
 */
export type BtCategory = "audio" | "keyboard" | "mouse" | "other";

/** 一台已连接的蓝牙设备。字段名 = Rust 侧 `#[serde(rename_all = "camelCase")]` 的输出。 */
export interface BtDevice {
  /** 稳定唯一标识（v-for 的 key；**不是**设备名）。 */
  id: string;
  name: string;
  kind: BtKind;
  category: BtCategory;
  /** 0–100；读不到为 null（需求强制：未知 ≠ 0）。 */
  batteryPercent: number | null;
  batterySource: BatterySource;
  isAudio: boolean;
}

/** 蓝牙子系统的整体状态（三态分离，界面按它给不同提示）。 */
export type BtStatus =
  | { status: "ok"; devices: BtDevice[] }
  | { status: "poweredOff" }
  | { status: "noAdapter" }
  | { status: "error"; message: string };

/** 订阅额度的单个配额窗口。与 Rust 侧 `QuotaWindow` 对齐。
 *
 *  ★ opencode Go 的用量接口返回三个独立重置的窗口（滚动 5h / 本周 / 本月），
 *    每个窗口各自有剩余百分比与重置时刻 —— 只看单一总额会丢掉
 *    "哪个窗口先耗尽、什么时候恢复"这个关键信息。
 */
export interface QuotaWindow {
  /** 窗口标签（"滚动" / "本周" / "本月"），由后端定好。 */
  label: string;
  /** **剩余**百分比 0–100（接口给的是已用，后端已换算）。 */
  remainingPercent: number;
  /** 该窗口的重置时刻（原样字符串）。 */
  resetsAt: string | null;
  /** 接口给的窗口状态（"ok" / 超额提示等）。 */
  status: string | null;
}

/** opencode-go 额度状态。 */
export type QuotaStatus =
  | {
      status: "ok";
      remaining: number;
      total: number | null;
      used: number | null;
      unit: string;
      resetsAt: string | null;
      raw: string;
      /** 多窗口接口（opencode Go）才有；旧接口为空数组。 */
      windows: QuotaWindow[];
    }
  | { status: "notConfigured" }
  | { status: "unauthorized"; message: string }
  | { status: "network"; message: string }
  | { status: "apiError"; message: string };

/** 一次性快照。时间戳用 Unix 毫秒（与 `Date.now()` 同坐标系）。 */
export interface InsightSnapshot {
  bluetooth: BtStatus;
  bluetoothAtMs: number;
  quota: QuotaStatus;
  quotaAtMs: number;
}

/** `bt_refresh` 的返回值。 */
export interface RefreshOutcome {
  throttled: boolean;
  bluetooth: BtStatus;
}

/** 额度配置的展示视图（Key 打码）。 */
export interface QuotaConfigView {
  hasKey: boolean;
  keyHint: string;
  /** 实际使用的接口地址（用户留空时为内置默认端点）。 */
  endpoint: string;
  /** 该地址是否来自内置默认值。 */
  endpointIsDefault: boolean;
}

export const insightApi = {
  /** 读当前快照（纯读缓存，不触发扫描）。挂载时调用一次。 */
  current: () => invoke<InsightSnapshot>("insight_current"),

  /** 查蓝牙设备。受 Rust 侧 10 秒节流闸门约束，被节流时返回缓存值。 */
  btDevices: () => invoke<BtStatus>("bt_devices"),

  /** 用户显式刷新蓝牙；`throttled` 告诉前端"刚刚才扫过"。 */
  btRefresh: () => invoke<RefreshOutcome>("bt_refresh"),

  /** 查询额度（真发 HTTP）。 */
  quotaQuery: () => invoke<QuotaStatus>("quota_query"),

  /** 读额度配置（Key 打码）。 */
  quotaConfigGet: () => invoke<QuotaConfigView>("quota_config_get"),

  /** 写额度配置并立刻查询一次，返回值即最新额度状态。 */
  quotaConfigSet: (patch: { apiKey?: string | null; endpoint?: string | null }) =>
    invoke<QuotaStatus>("quota_config_set", {
      apiKey: patch.apiKey ?? null,
      endpoint: patch.endpoint ?? null,
    }),

  /** 开关托盘旁的悬浮信息窗（overlay 窗口）。 */
  overlayToggle: (visible: boolean) => invoke<boolean>("overlay_toggle", { visible }),
};
