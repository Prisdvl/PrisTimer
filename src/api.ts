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
