<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { timerApi, type PomodoroStatus, type Recovered, type TimerSnapshot } from "./api";
import { formatClock, formatDuration, toDayKey } from "./date";
import { toast } from "./composables/useToast";
import { useTheme } from "./composables/useTheme";
import { usePomodoro, PHASE_LABEL } from "./composables/usePomodoro";
import { useTags } from "./composables/useTags";
import { useMiniWindow } from "./composables/useMiniWindow";
import AnalogDial from "./components/AnalogDial.vue";
import IntroSplash from "./components/IntroSplash.vue";
import MiniWidget from "./components/MiniWidget.vue";
import ParticleField from "./components/ParticleField.vue";
import PomodoroPanel from "./components/PomodoroPanel.vue";
import SmokeField from "./components/SmokeField.vue";
import StatsView from "./components/StatsView.vue";
import StatusBar from "./components/StatusBar.vue";
import TagRow from "./components/TagRow.vue";
import ThemeMenu from "./components/ThemeMenu.vue";
import TitleBar from "./components/TitleBar.vue";
import ToastHost from "./components/ToastHost.vue";
import TypewriterHint from "./components/TypewriterHint.vue";

type Tab = "timer" | "stats";

const tab = ref<Tab>("timer");

// ---------------------------------------------------------------------------
// 顶部滚动进度条：滚动条已全局取消（glass.css），"滚到哪里了"改由窗口
// 顶缘这条 2.5px 细线表达。监听全部 passive + rAF 节流 —— 滚动路径上
// 不做同步布局读取，也不每事件都量。
// ---------------------------------------------------------------------------
const scrollProgress = ref(0);
const scrollable = ref(false);
let scrollRaf = 0;

function measureScroll(): void {
  const el = document.scrollingElement;
  if (!el) return;
  const max = el.scrollHeight - el.clientHeight;
  scrollable.value = max > 4;
  scrollProgress.value = max > 4 ? Math.min(1, Math.max(0, el.scrollTop / max)) : 0;
}

function onScroll(): void {
  if (scrollRaf) return;
  scrollRaf = requestAnimationFrame(() => {
    scrollRaf = 0;
    measureScroll();
  });
}

/** 切页会改写 scrollHeight（统计页远高于计时页）：过渡结束后再量一次。 */
watch(tab, () => {
  void nextTick(measureScroll);
  window.setTimeout(measureScroll, 600);
});

// ---------------------------------------------------------------------------
// 启动动画 / 主题 / 冥想模式 —— 三个全局 UI 开关。
// ---------------------------------------------------------------------------

/** 启动画面（Scramble 标题 + 粒子）：播完一次就摘掉，全程 ≤0.5s。 */
const introDone = ref(false);

// 背景主题：纯逻辑在 composables/useTheme.ts（模块级单例，与 ThemeMenu 共享）。
// 菜单开合是 ThemeMenu 的私有 UI 态，App 不再关心。
const { theme, syncTheme, simpleMode } = useTheme();

/** 冥想模式：呼吸圆环 + 慢速粒子背景。纯氛围层，不改变计时行为。 */
const MEDITATION_KEY = "pristimer.meditation";
const meditation = ref(localStorage.getItem(MEDITATION_KEY) === "1");
watch(meditation, (v) => {
  try {
    localStorage.setItem(MEDITATION_KEY, v ? "1" : "0");
  } catch {
    /* 同上 */
  }
});

/** 与 Rust 侧 MIN_SESSION_MS 对齐：短于它的会话在写入侧就被丢弃。 */
const MIN_SESSION_MS = 10_000;

const snapshot = ref<TimerSnapshot>({
  state: "idle",
  elapsedMs: 0,
  remainingMs: null,
  limitMs: null,
});

/** 启动时恢复出来的未完成会话。 */
const recovered = ref<Recovered | null>(null);
const bannerDismissed = ref(false);

// 番茄钟状态：纯逻辑在 composables/usePomodoro.ts（模块级单例，与 PomodoroPanel 共享）。
// 阶段条/设置面板/序列卡归 PomodoroPanel，App 只留侧栏与开关要用的部分。
const { pomodoro, pomoConfig, pomoSettingsOpen, syncDraft, togglePomodoro, phaseAccent } = usePomodoro();

// ---------------------------------------------------------------------------
// 键盘快捷键（应用内）：空格 = 开始/暂停，Esc = 退出迷你态 / 关设置面板。
//
// 刻意**不做**系统级全局热键：空格若被注册成全局热键，会劫持所有其他
// 应用里的打字与刷题。窗口聚焦时的空格语义与主按钮完全一致
// （running → 暂停，其余 → 开始），输入框聚焦时自动让路。
//
// ★ 「去浏览器化」第二层防线。
//
//   第一层在 Rust 侧：`shell.rs` 通过 WebView2 原生设置关掉了浏览器
//   专属加速键（F5 / Ctrl+R / F12 / Ctrl+P / Ctrl+F / 缩放 / 前进后退），
//   那些按键在**到达 JS 之前**就被 WebView2 吞掉了，这里拦不到也不需要拦。
//
//   这一层只兜两条边：
//     · WebView2 运行时过老、原生设置没生效时（Rust 侧会记日志），
//       F5 / F11 / F12 等仍可能到达页面 —— 在这里补 preventDefault；
//     · 原生设置清单里**不包含**的浏览器行为（F11 全屏等）。
// ---------------------------------------------------------------------------
function onKeydown(e: KeyboardEvent): void {
  // ---- 浏览器独有快捷键：一律吞掉 ----
  // 注意：在 processor 里，`e.key` 需要从 `e.key` 读（KeyboardEvent），
  // 不能依赖 deprecated 的 `keyCode`。
  const browserKey =
    e.key === "F5" ||
    e.key === "F11" ||
    e.key === "F12" ||
    (e.ctrlKey && e.key.toLowerCase() === "r") || // Ctrl+R 刷新
    (e.ctrlKey && e.key.toLowerCase() === "p") || // Ctrl+P 打印
    (e.ctrlKey && e.key.toLowerCase() === "f") || // Ctrl+F 查找
    (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "i") || // Ctrl+Shift+I 检查
    (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "c"); // Ctrl+Shift+C 检查元素
  if (browserKey) {
    e.preventDefault();
    e.stopPropagation();
    return;
  }

  if (e.ctrlKey || e.altKey || e.metaKey) return;
  const target = e.target as HTMLElement | null;
  if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)) {
    return;
  }
  if (e.key === " ") {
    // 空格默认滚动页面 —— 这里接管为开始/暂停，必须 preventDefault
    e.preventDefault();
    if (isRunning.value) void timerApi.pause();
    else void timerApi.start();
    return;
  }
  if (e.key === "Escape") {
    // 主题菜单的 Esc 由 ThemeMenu 自己处理，这里管剩下的。
    // 第 23 轮：沉浸模式优先退出 —— 沉浸下没有别的可见出口依赖它。
    if (immersive.value) immersive.value = false;
    else if (uiConsoleOpen.value) uiConsoleOpen.value = false;
    else if (mini.value) void toggleMini();
    else if (pomoSettingsOpen.value) pomoSettingsOpen.value = false;
  }
}

// ---------------------------------------------------------------------------
// 组件显隐控制台 + 沉浸模式（第 23 轮 R7/R8/R9）。
//
// 显隐分两层：
//   · uiFlags —— 「隐藏但一键找回」：标签栏 / 快捷时长区 / 侧栏状态 / 使用提示，
//     右下角控制台随时可再开；
//   · immersive —— 「只留时钟与动效」：隐藏包括 tabs / 控制按钮在内的一切外壳，
//     入口在控制台里，出口是右上角幽灵钮或 Esc（不占主要位置，不影响观感）。
// 全部 localStorage 持久化，与主题/简约模式互相独立。
// ---------------------------------------------------------------------------

const UI_FLAG_KEYS = {
  tags: "pristimer.ui.tags",
  presets: "pristimer.ui.presets",
  rail: "pristimer.ui.rail",
  tips: "pristimer.ui.tips",
} as const;

function loadFlag(key: string, dflt = true): boolean {
  try {
    const v = localStorage.getItem(key);
    return v === null ? dflt : v === "1";
  } catch {
    return dflt;
  }
}

const uiFlags = reactive({
  tags: loadFlag(UI_FLAG_KEYS.tags),
  presets: loadFlag(UI_FLAG_KEYS.presets),
  rail: loadFlag(UI_FLAG_KEYS.rail),
  tips: loadFlag(UI_FLAG_KEYS.tips),
});

const UI_ROWS: Array<{ key: keyof typeof UI_FLAG_KEYS; label: string }> = [
  { key: "tags", label: "标签栏" },
  { key: "presets", label: "快捷时长区" },
  { key: "rail", label: "侧栏状态" },
  { key: "tips", label: "使用提示" },
];

watch(uiFlags, () => {
  try {
    for (const [k, key] of Object.entries(UI_FLAG_KEYS)) {
      localStorage.setItem(key, uiFlags[k as keyof typeof uiFlags] ? "1" : "0");
    }
  } catch {
    /* 写不进去就本次会话生效 */
  }
});

const immersive = ref(loadFlag("pristimer.immersive", false));
watch(immersive, (on) => {
  try {
    localStorage.setItem("pristimer.immersive", on ? "1" : "0");
  } catch {
    /* 同上 */
  }
  if (on) {
    // 沉浸只对计时页有意义；若正停在统计页，先切回去
    tab.value = "timer";
    uiConsoleOpen.value = false;
  }
});

const uiConsoleOpen = ref(false);

// ---------------------------------------------------------------------------
// 计时页底部的使用提示（Tabs 滑动指示器）。
// ---------------------------------------------------------------------------

const TIPS: Array<{ label: string; text: string }> = [
  {
    label: "番茄节奏",
    text: "开启番茄钟后自动循环：专注 → 短休，N 轮后一次长休。点上方胶囊展开面板可改时长与轮数。",
  },
  {
    label: "快捷时长",
    text: "点档位即设定倒计时，「正计时」不限时长；自定义可输入 1–1440 分钟，回车或点「设定」生效。",
  },
  {
    label: "标签与统计",
    text: "开始前选好科目标签，会话自动归入该科目；双击标签可重命名，统计页里按科目查看与合并。",
  },
];
const tipIdx = ref(0);
// PHASE_LABEL / togglePomodoro / 圆点与阶段色：见 composables/usePomodoro.ts

const STATE_LABEL: Record<string, string> = {
  idle: "空闲",
  running: "计时中",
  paused: "已暂停",
  finished: "已完成",
};

// ---------------------------------------------------------------------------
// 侧栏状态栏（Spotlight 聚光灯卡片）：今日专注时长 + 完成的番茄数。
// ---------------------------------------------------------------------------

const todayMs = ref(0);
const todayCount = ref(0);
const pomodoroDone = computed(() => pomodoro.value?.completedFocus ?? 0);

async function loadToday(): Promise<void> {
  try {
    const key = toDayKey(new Date());
    const rows = await timerApi.dailyStats(key, key);
    todayMs.value = rows[0]?.totalMs ?? 0;
    todayCount.value = rows[0]?.sessionCount ?? 0;
  } catch {
    /* 状态栏是锦上添花，拉不到就保持旧值 */
  }
}

/** Spotlight 光斑跟随鼠标：把落点写进 CSS 变量，渐变圆心贴着指针走。 */
function spotMove(e: MouseEvent): void {
  const el = e.currentTarget as HTMLElement | null;
  if (!el) return;
  const r = el.getBoundingClientRect();
  el.style.setProperty("--sx", `${e.clientX - r.left}px`);
  el.style.setProperty("--sy", `${e.clientY - r.top}px`);
}

// ---------------------------------------------------------------------------
// 专注开始时的打字机提示（Typewriter）：顶端逐字浮现，停留后自己淡去。
// ---------------------------------------------------------------------------

const HINTS = ["保持专注，慢慢来", "一次只做一件事", "此刻即是全部", "深呼吸，开始吧"];
const focusHint = ref<string | null>(null);

// ---------------------------------------------------------------------------
// 科目标签：下一条会话开始前选好，落库那一刻固定（会话是历史事实）。
//
// 标签集合归用户管：预设只是**初始值**，之后可以新增（输入框回车）、双击重命名、
// 点 × 删除。集合存本地 —— 它是 UI 偏好，不是需要跨设备对齐的数据；
// 而历史会话上的 tag 是既成事实，改名只影响"下一条"，不回改历史。
// ---------------------------------------------------------------------------

// 科目标签：纯逻辑在 composables/useTags.ts（模块级单例，与 TagRow 共享）。
// App 只留迷你组件要展示的当前选中科目。
const { selectedTag } = useTags();

/** 四种状态各配一个强调色，盘面、圆点、主按钮共用。
 *  番茄休息阶段优先用阶段色（蓝 / 紫），让「放松时段」有别于专注。 */
const accent = computed(() => {
  const overriding =
    phaseAccent.value &&
    snapshot.value.state !== "idle" &&
    snapshot.value.state !== "finished";
  if (overriding) return phaseAccent.value;
  switch (snapshot.value.state) {
    case "running":
      return "#3ecf8e";
    case "paused":
      return "#e8b64c";
    case "finished":
      return "#5aa7ff";
    default:
      return "#4a5160";
  }
});

// 倒计时显示剩余，正计时显示已用。
const display = computed(() =>
  formatClock(snapshot.value.remainingMs ?? snapshot.value.elapsedMs),
);

/** 超过 1 小时后是 7 位字符，字号自动降一档，避免撑破盘心。 */
const isLongFormat = computed(() => display.value.length > 5);

/**
 * 把读数拆成「定宽数字位」，每位一格。
 *
 * ★ 为什么不能直接渲染整串再居中：
 *   `01:11` 与 `01:20` 在比例字体下宽度并不严格相等（即使开了 tabular-nums，
 *   非整数字号带来的亚像素取整也会让整串宽度飘移），而读数原本是
 *   「绝对定位 + translate(-50%)」居中 —— 整串一宽，**每一位都会跟着左右微移**，
 *   于是数字看起来一直在抖。
 *
 * 现在每位是一个固定宽度的槽（`.cell`），槽宽由 CSS 钉死，槽内数字换值时做一次
 * 上下翻页。位置完全由 flex 布局决定，**与字形宽度彻底解耦**。
 *
 * key 从右往左编号：低位（秒的个位）永远复用同一个 DOM 节点 —— 否则"位数变了"
 * 会把整排节点重新挂载，翻页动画就断了。
 */
const cells = computed(() => {
  const text = display.value;
  const total = text.length;
  return [...text].map((ch, i) => ({
    key: total - 1 - i,
    digit: ch === ":" ? null : Number(ch),
  }));
});

const isRunning = computed(() => snapshot.value.state === "running");
const isIdle = computed(() => snapshot.value.state === "idle");

/**
 * 指针表的输入：**已进行**的毫秒数。
 * 倒计时取 limit - remaining，这样指针始终朝前走（与机械秒表同向），
 * 剩多少交给外圈那道细弧表达。
 */
const dialElapsedMs = computed(() => {
  const { limitMs, remainingMs, elapsedMs } = snapshot.value;
  if (limitMs !== null && limitMs > 0 && remainingMs !== null) {
    return Math.max(0, limitMs - remainingMs);
  }
  return elapsedMs;
});

/** 主按钮三种形态。文案差别大（2 字 / 4 字），靠网格叠放自适应宽度。 */
const PRIMARY_LABELS = ["开始", "继续", "重新开始"] as const;
const primaryLabel = computed(() => {
  switch (snapshot.value.state) {
    case "paused":
      return "继续";
    case "finished":
      return "重新开始";
    default:
      return "开始";
  }
});

// ---------------------------------------------------------------------------
// 倒计时进度环（细弧，只在有目标时出现）。
// ---------------------------------------------------------------------------

const ARC_PROGRESS = computed(() => {
  const { limitMs, remainingMs } = snapshot.value;
  if (!limitMs || limitMs <= 0) return null;
  const done = limitMs - (remainingMs ?? limitMs);
  return Math.min(1, Math.max(0, done / limitMs));
});

// ---------------------------------------------------------------------------
// 快捷时长档。引擎规定 set_limit 只在 Idle 生效，所以非空闲时整排禁用。
// ---------------------------------------------------------------------------

const PRESETS: Array<{ label: string; limitMs: number | null }> = [
  { label: "正计时", limitMs: null },
  { label: "25 分", limitMs: 25 * 60_000 },
  { label: "45 分", limitMs: 45 * 60_000 },
  { label: "60 分", limitMs: 60 * 60_000 },
];

const activePreset = computed(() => {
  const { limitMs } = snapshot.value;
  return PRESETS.findIndex((p) => p.limitMs === limitMs);
});

// 自定义时长：引擎规定 set_limit 只在 Idle 生效，非空闲时输入同样禁用。
const customMinutes = ref(30);
const isCustomActive = computed(() => {
  const { limitMs } = snapshot.value;
  return limitMs !== null && activePreset.value === -1;
});

/** 设定成功的反馈态：按钮变 ✓、整组脉冲一次。 */
const customApplied = ref(false);
let appliedTimer: number | undefined;

/** 输入框宽度跟着位数走。
 *
 *  ★ 原来用 `ch`（"0" 的 advance 宽度）配 `Math.max(2, …)` 下限：2ch 在 0.8rem
 *  字号下只有约 10px 内容区，而数字字形宽约 0.62em —— "30" 两个字根本放不下，
 *  外层 `overflow: hidden` 就把它裁掉了。改成按 em 精算并留一点余量。 */
const inputWidth = computed(() => {
  const len = Math.max(2, Math.min(4, String(customMinutes.value).length));
  return `${(len * 0.72).toFixed(2)}em`;
});

/** 输入即截断：只留数字、最多 4 位（1440 = 一天的分钟数）。 */
function onCustomInput(event: Event): void {
  const el = event.target as HTMLInputElement;
  const digits = el.value.replace(/\D/g, "").slice(0, 4);
  if (digits !== el.value) {
    el.value = digits;
  }
  customMinutes.value = digits === "" ? 0 : Number(digits);
}

function applyCustom(): void {
  const minutes = Math.floor(customMinutes.value);
  if (!Number.isFinite(minutes) || minutes < 1 || minutes > 1440) {
    toast.error("时长超出范围", "请输入 1–1440 之间的分钟数");
    return;
  }
  customMinutes.value = minutes;
  timerApi.setLimit(minutes * 60_000);

  // 重启动画：先摘掉类，下一帧再加回，连续点击也能每次都弹。
  customApplied.value = false;
  requestAnimationFrame(() => {
    customApplied.value = true;
  });
  clearTimeout(appliedTimer);
  appliedTimer = window.setTimeout(() => {
    customApplied.value = false;
  }, 700);
}

/** 失焦时钳制到合法区间，避免留下半截输入。 */
function clampCustom(): void {
  const value = Math.floor(customMinutes.value);
  customMinutes.value = Number.isFinite(value) ? Math.min(1440, Math.max(1, value)) : 30;
}

/**
 * 重置 / 中途放弃：先把当前这条会话的时长读出来再下发重置 ——
 * 重置之后快照归零，就再也问不出"刚才跑了多久"了。
 * 提示语按是否够格入库分两种，让用户知道这次有没有被记进统计。
 */
function resetTimer(): void {
  const before = snapshot.value;
  const spent = dialElapsedMs.value;
  timerApi.reset();
  if (before.state === "idle") return;
  if (spent >= MIN_SESSION_MS) {
    toast.success("已记入统计", `本次专注 ${formatDuration(spent)}`);
  } else {
    toast.info("本次时长过短，未记录", "不足 10 秒的会话按误触处理，不进统计");
  }
}

/** 关闭窗口 = 收进托盘继续计时，用户第一次点会疑惑，给一次说明。 */
function onWindowClose(): void {
  toast.info("已收进托盘", "计时会在后台继续，点托盘图标可以叫回来");
}

// 迷你模式与窗口几何：纯逻辑在 composables/useMiniWindow.ts
const {
  mini,
  morphOut,
  geomAnimating,
  toggleMini,
  syncMiniClass,
  closeToTray,
  enterMiniWindow,
  ensureNormalMinSize,
  watchWindowPersistence,
} = useMiniWindow();

/** 迷你组件的状态文案：番茄开启时优先显示阶段（该干劲还是该放松）。 */
const miniLabel = computed(() => {
  if (pomodoro.value?.enabled) return PHASE_LABEL[pomodoro.value.phase];
  return STATE_LABEL[snapshot.value.state] ?? snapshot.value.state;
});
// 「收进托盘」已在 composables/useMiniWindow.ts（closeToTray）

const showBanner = computed(() => recovered.value !== null && !bannerDismissed.value);

let unlisten: UnlistenFn | null = null;
let unlistenPomodoro: UnlistenFn | null = null;

onMounted(async () => {
  document.addEventListener("keydown", onKeydown);
  document.addEventListener("contextmenu", onContextmenu);
  document.addEventListener("dragover", onGlobalDragOver);
  document.addEventListener("drop", onGlobalDrop);
  // 窗口几何已由 Rust 在 show 之前摆好（见 lib.rs 的 place_main_window），
  // 前端不再参与启动期恢复 —— 这里只补运行时约束。
  syncMiniClass();
  syncTheme();
  void loadToday();
  if (mini.value) {
    // 以迷你形态醒来：Rust 已经摆好了尺寸与位置，这里再走一遍是幂等的，
    // 顺便自愈「窗口状态文件丢了但 localStorage 还记着迷你」的情况。
    await enterMiniWindow();
  } else {
    // 常规形态的最小尺寸在运行时补（配置里不静态写死，给迷你小窗让路）
    await ensureNormalMinSize();
  }

  // 窗口几何变动 → 防抖落盘（迷你态只记位置，全尺寸记整套状态）
  await watchWindowPersistence();

  // 顶部滚动进度条：滚动/尺寸变化都只触发 rAF 节流的测量
  window.addEventListener("scroll", onScroll, { passive: true });
  window.addEventListener("resize", onScroll, { passive: true });
  measureScroll();

  // 先订阅，再拉当前值 —— 顺序反了会丢掉订阅与拉取之间到达的快照。
  unlisten = await listen<TimerSnapshot>("timer:update", (event) => {
    snapshot.value = event.payload;
  });

  unlistenPomodoro = await listen<PomodoroStatus>("pomodoro:update", (event) => {
    pomodoro.value = event.payload;
  });

  try {
    // Rust 在前端加载完成前就已推送过第一帧（事件不补发），
    // 恢复出来的暂停态就丢在那次推送里，所以这里必须主动拉一次。
    const current = await timerApi.current();
    if (current !== null) {
      snapshot.value = current;
    }
    recovered.value = await timerApi.recovered();
    pomodoro.value = await timerApi.pomodoroCurrent();
    const config = await timerApi.pomodoroConfigGet();
    pomoConfig.value = config;
    syncDraft(config);
    selectedTag.value = await timerApi.tagCurrent();
  } catch (err) {
    console.error("读取恢复会话失败", err);
  }
});

/** 倒计时跑到头：Rust 已发系统通知，界面这边补一条同款 toast 收束注意力。
 *  同时兼顾两件小事：开始专注时顶端浮出打字机提示；会话结算后刷新侧栏的
 *  「今日专注」（落库有一拍延迟，等 400ms 再拉）。 */
watch(
  () => snapshot.value.state,
  (now, prev) => {
    if (now === "finished" && prev !== undefined && prev !== "finished") {
      const { limitMs } = snapshot.value;
      toast.success("计时完成", limitMs ? `目标 ${formatDuration(limitMs)}` : undefined);
    }
    if (now === "running" && prev !== undefined && prev !== "running") {
      focusHint.value = HINTS[Math.floor(Math.random() * HINTS.length)];
    }
    if (prev !== undefined && ((now === "finished" && prev !== "finished") || (now === "idle" && prev !== "idle"))) {
      window.setTimeout(() => void loadToday(), 400);
    }
  },
);

// ---------------------------------------------------------------------------
// 右键菜单缺陷修复：WebView2 默认把"网页右键"（重新加载 / 检查元素等）带进
// 桌面应用，出戏且暴露调试面。全局拦截 contextmenu —— 唯一例外是输入框
// （input / textarea / contenteditable），那里保留系统菜单（复制粘贴是功能）。
// ---------------------------------------------------------------------------
function onContextmenu(e: MouseEvent): void {
  const target = e.target as HTMLElement | null;
  if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)) {
    return;
  }
  e.preventDefault();
}

// ---------------------------------------------------------------------------
// 全局拖放守卫：窗口已开启原生 DnD 通路（tauri.conf 的 dragDropEnabled=false，
// 番茄序列卡片的 HTML5 拖拽才能工作），但应用并不接收外部文件 ——
// 拦掉 dragover/drop 的浏览器默认行为（拖文件进窗口默认 = 跳转到该文件），
// 窗口对文件的拖放回到"什么都不发生"。卡片间的拖拽有各自的 drop 处理器：
// 这里只是补一层 preventDefault，不参与任何排序逻辑。
// ---------------------------------------------------------------------------
function onGlobalDragOver(e: DragEvent): void {
  e.preventDefault();
}
function onGlobalDrop(e: DragEvent): void {
  e.preventDefault();
}

onUnmounted(() => {
  document.removeEventListener("keydown", onKeydown);
  document.removeEventListener("contextmenu", onContextmenu);
  document.removeEventListener("dragover", onGlobalDragOver);
  document.removeEventListener("drop", onGlobalDrop);
  unlisten?.();
  unlistenPomodoro?.();
  clearTimeout(appliedTimer);
  if (scrollRaf) cancelAnimationFrame(scrollRaf);
  window.removeEventListener("scroll", onScroll);
  window.removeEventListener("resize", onScroll);
});
</script>

<template>
  <div class="app" :class="{ meditating: meditation }" :style="{ '--accent': accent }">
    <SmokeField v-if="!simpleMode" :paused="mini || geomAnimating" :theme="theme" :active="isRunning" />
    <!-- 顶部滚动进度条：替代已取消的右侧滚动条（fixed 于窗口顶缘） -->
    <span
      class="scroll-progress"
      :class="{ show: scrollable }"
      :style="{ transform: `scaleX(${scrollProgress})` }"
      aria-hidden="true"
    />
    <!-- 冥想模式：慢速粒子星图浮在烟雾之上、内容之下 -->
    <Transition name="fade">
      <div v-if="meditation" class="meditation-layer" aria-hidden="true">
        <ParticleField :count="42" :speed="0.4" rgb="132,120,255" :alpha="0.75" />
      </div>
    </Transition>
    <ToastHost />
    <!-- 沉浸模式唯一的常驻出口：右上角幽灵钮，平时近乎隐形，hover 才显形 -->
    <button
      v-if="immersive && !mini"
      class="immersive-exit"
      title="退出沉浸模式（Esc）"
      @click="immersive = false"
    >
      ×
    </button>
    <!-- 启动动画：Scramble 解码标题 + 粒子背景，~1.5s 后摘掉 -->
    <IntroSplash v-if="!introDone && !mini" @done="introDone = true" />

    <div class="shell" :class="{ mini, 'morph-out': morphOut }">
      <TitleBar v-if="!mini && !immersive" @close="onWindowClose" @mini="toggleMini" />

      <MiniWidget
        v-if="mini"        :display="display"
        :mini-label="miniLabel"
        :selected-tag="selectedTag"
        :is-running="isRunning"
        :progress="ARC_PROGRESS"
        @toggle="toggleMini"
        @tray="closeToTray"
      />

      <div v-else class="stage">
        <nav v-if="!immersive" class="tabs">
          <!-- 滑动的玻璃药丸：两个标签共用一块高光，切换时它平移过去 ——
               "一个按钮融进另一个按钮"在标签切换上最直观的表达 -->
          <span class="pill" :style="{ transform: `translateX(${tab === 'timer' ? 0 : 100}%)` }" />
          <button :class="{ active: tab === 'timer' }" @click="tab = 'timer'">计时</button>
          <button :class="{ active: tab === 'stats' }" @click="tab = 'stats'">统计</button>
        </nav>

        <Transition name="banner">
          <p v-if="showBanner" class="banner">
            <span class="banner-line" />
            <span class="banner-text">
              上次有 <b>{{ formatDuration(recovered!.elapsedMs) }}</b> 的未结束会话，已恢复为暂停状态。
              按「继续」接着计，或按「重置」把它记为一次记录。
            </span>
            <button class="dismiss" @click="bannerDismissed = true">知道了</button>
          </p>
        </Transition>

        <!-- 3D 翻页 + 模糊：page 改 flip，进场/出场都带一点 rotateY 与 blur -->
        <Transition name="flip" mode="out-in">
          <StatsView v-if="tab === 'stats'" key="stats" />

          <main v-else key="timer" class="timer">
            <!-- 开始专注瞬间的打字机提示（浮在计时页顶端，不占布局） -->
            <div v-if="!immersive" class="focus-hint" aria-live="polite">
              <TypewriterHint v-if="focusHint" :text="focusHint" @done="focusHint = null" />
            </div>

            <!-- 侧栏状态栏（Spotlight）：今日专注 + 番茄数。绝对定位不挤占表盘 -->
            <aside v-if="uiFlags.rail && !immersive" class="rail" aria-label="今日状态">
              <div class="spot" @mousemove="spotMove">
                <span class="spot-light" />
                <p class="spot-label">今日专注</p>
                <p class="spot-value">{{ formatDuration(todayMs) }}</p>
                <p class="spot-sub">{{ todayCount }} 次会话</p>
              </div>
              <div class="spot" @mousemove="spotMove">
                <span class="spot-light" />
                <p class="spot-label">番茄钟</p>
                <p class="spot-value">{{ pomodoroDone }}</p>
                <p class="spot-sub">{{ pomodoro?.enabled ? "已完成" : "未开启" }}</p>
              </div>
            </aside>

            <AnalogDial
              :elapsed-ms="dialElapsedMs"
              :progress="ARC_PROGRESS"
              :state="snapshot.state"
              :theme="theme"
              :simple="simpleMode"
            >
              <div class="readout">
                <div class="clock-window">
                  <p class="clock" :class="{ long: isLongFormat }" :aria-label="display">
                    <!-- 逐位渲染：每个字符占一个固定宽度的槽，槽内换值时上下翻页。
                         不用 out-in 模式（那会先清空再进场，每秒闪一次）；
                         进出两个数字在同一格里叠放，交叉滚动才是无缝的。
                         冒号也是一个槽（但更窄、更低透明度、运行时呼吸）。 -->
                    <span
                      v-for="cell in cells"
                      :key="cell.key"
                      class="cell"
                      :class="{ sep: cell.digit === null }"
                    >
                      <template v-if="cell.digit === null">:</template>
                      <Transition v-else name="digit">
                        <span :key="cell.digit" class="d">{{ cell.digit }}</span>
                      </Transition>
                    </span>
                  </p>
                  <p class="state">
                    <i class="dot" />{{ STATE_LABEL[snapshot.state] ?? snapshot.state }}
                  </p>
                </div>
              </div>
            </AnalogDial>

            <div v-if="!immersive" class="controls">
              <button
                class="action liquid"
                :class="{ live: !isRunning }"
                :disabled="isRunning"
                @click="timerApi.start()"
              >
                <!-- Liquid：三粒流光气泡在按钮里慢漂，hover 时上浮发光 -->
                <span class="blob" />
                <span class="blob" />
                <span class="blob" />
                <!-- 三种形态共用一个网格单元：按钮宽度自动取最宽的"重新开始"，
                     切换时旧文案失焦缩小淡出、新文案从下方浮起 —— 形态融合 -->
                <span class="labels">
                  <span
                    v-for="label in PRIMARY_LABELS"
                    :key="label"
                    :class="{ show: label === primaryLabel }"
                  >
                    {{ label }}
                  </span>
                </span>
              </button>
              <button
                class="action"
                :class="{ live: isRunning }"
                :disabled="!isRunning"
                @click="timerApi.pause()"
              >
                <span class="labels"><span class="show">暂停</span></span>
              </button>
              <button class="ghost" @click="resetTimer">重置</button>
            </div>

            <TagRow v-if="uiFlags.tags && !immersive" />

            <PomodoroPanel v-if="!immersive" />

            <div v-if="uiFlags.presets && !immersive" class="presets">
              <button
                class="pomo-toggle"
                :class="{ active: pomodoro?.enabled }"
                @click="togglePomodoro"
              >
                番茄钟
              </button>
              <button
                v-for="(preset, index) in PRESETS"
                :key="preset.label"
                :class="{ active: activePreset === index }"
                :disabled="!isIdle"
                @click="timerApi.setLimit(preset.limitMs)"
              >
                {{ preset.label }}
              </button>

              <div class="custom" :class="{ active: isCustomActive, pulse: customApplied }">
                <input
                  :value="customMinutes"
                  type="number"
                  min="1"
                  max="1440"
                  :style="{ width: inputWidth }"
                  :disabled="!isIdle"
                  @input="onCustomInput"
                  @blur="clampCustom"
                  @keyup.enter="applyCustom"
                />
                <span class="unit">分钟</span>
                <button
                  :class="{ done: customApplied }"
                  :disabled="!isIdle"
                  @click="applyCustom"
                >
                  {{ customApplied ? "✓" : "设定" }}
                </button>
              </div>

              <!-- 氛围控制：冥想模式开关 + 主题色板 -->
              <span class="aux">
                <button
                  class="meditate"
                  :class="{ active: meditation }"
                  title="冥想模式：呼吸圆环 + 慢速粒子背景"
                  @click="meditation = !meditation"
                >
                  冥想
                </button>
                <ThemeMenu />
              </span>
            </div>

            <!-- 使用提示：Tabs 滑动指示器 + 内容淡入 -->
            <div v-if="uiFlags.tips && !immersive" class="tips">
              <div class="tips-nav">
                <span
                  class="tips-indicator"
                  :style="{ transform: `translateX(${tipIdx * 100}%)` }"
                />
                <button
                  v-for="(t, i) in TIPS"
                  :key="t.label"
                  :class="{ active: tipIdx === i }"
                  @click="tipIdx = i"
                >
                  {{ t.label }}
                </button>
              </div>
              <Transition name="tipslide" mode="out-in">
                <p :key="tipIdx" class="tips-body">{{ TIPS[tipIdx].text }}</p>
              </Transition>
            </div>
          </main>
        </Transition>
      </div>
      <!-- 右下角组件控制台：显隐组件 + 沉浸模式入口（迷你态/沉浸态不显示） -->
      <div v-if="!mini && !immersive" class="ui-console">
        <Transition name="fade">
          <div v-if="uiConsoleOpen" class="ui-panel" @click.stop>
            <p class="ui-panel-title">显示组件</p>
            <label v-for="row in UI_ROWS" :key="row.key" class="ui-row">
              <span>{{ row.label }}</span>
              <i
                class="switch"
                :class="{ on: uiFlags[row.key] }"
                role="switch"
                :aria-checked="uiFlags[row.key]"
                @click.prevent="uiFlags[row.key] = !uiFlags[row.key]"
              />
            </label>
            <button class="immersive-enter" @click="immersive = true">
              沉浸模式
            </button>
          </div>
        </Transition>
        <button
          class="ui-fab"
          title="显示组件 / 沉浸模式"
          @click.stop="uiConsoleOpen = !uiConsoleOpen"
        >
          <span class="ui-fab-icon" aria-hidden="true" />
        </button>
      </div>

      <!-- 底部系统信息状态栏：蓝牙设备 + opencode-go 额度。
           常规形态且非沉浸时显示（迷你组件有自己的紧凑布局，不塞这条）。 -->
      <StatusBar v-if="!mini && !immersive" />
    </div>
  </div>
</template>

<style scoped>
.app {
  position: relative;
  min-height: 100vh;
}

/* 顶部滚动进度条：滚动条取消后的位置指示。transform 由脚本逐帧写入
   （不做 transition —— 进度要跟手），淡入淡出走 opacity。 */
.scroll-progress {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  z-index: calc(var(--z-toast) + 1);
  height: 2.5px;
  background: linear-gradient(
    90deg,
    color-mix(in srgb, var(--accent) 62%, transparent),
    var(--accent)
  );
  box-shadow: 0 0 10px color-mix(in srgb, var(--accent) 42%, transparent);
  transform-origin: left center;
  transform: scaleX(0);
  opacity: 0;
  transition: opacity 0.35s ease;
  pointer-events: none;
}
.scroll-progress.show {
  opacity: 1;
}

/* 内容层：压在烟雾之上（烟雾是 fixed + z-index 0） */
.shell {
  position: relative;
  z-index: var(--z-content);
  display: flex;
  flex-direction: column;
  min-height: 100vh;
  /* ★ 底部为固定的信息状态栏（实测约 28–30px）留出滚动空间。
     状态栏是 `position: fixed` —— 它不参与文档流，不给这里留白的话
     页面最底部的「使用提示 / 统计页末块」会被压在它下面（看得见但
     点不着，因为状态栏在上层且盖住了）。留 38px 是状态栏高度 +
     一点余量，保证滚到底时最后一行内容完全露出来。 */
  padding-bottom: 38px;
  /* 形态切换的内容淡出（morph-out）：几何动画期间模板不可见，到位后切模板 */
  transition: opacity 0.18s ease;
}
.shell.morph-out {
  opacity: 0;
}

/* ------------------------------------------------------------- 迷你模式 */
/* 整窗是一枚 264×96 的玻璃"组件"：时间 + 状态 + 开始/暂停 + 还原。
   全屏 data-tauri-drag-region 可拖到任意位置，置顶浮在其它窗口之上。
   `position: fixed` + `inset: 0` 是为了**脱离文档流铺满视口**：曾经用
   `flex: 1` 挂在 .shell 里时，宽度会被 classic 滚动条的预留槽挤窄，露出的
   深色底在小窗上就是一条黑边（滚动条如今已全局取消，但 fixed 仍是更稳的
   边界定义 —— 视口即边界，一滴不漏）。 */
.stage {
  flex: 1;
  display: flex;
  flex-direction: column;
  padding: 1.1rem 1.5rem 2rem;
  /* 3D 翻页的透视源：只影响切页动画的子元素（标题栏在 .stage 外，不受影响）。
     注意不能用 transform 实现 —— 那会另立包含块，破坏内部玻璃层的合成。 */
  perspective: 1400px;
  /* 从迷你还原时窗口会由小骤然变大，内容若同时"啪"地出现会很跳。
     给一层淡入把两个动作连起来。（只碰 opacity —— 给这个容器加 transform
     会另立一个包含块，影响内部 sticky 标题栏与玻璃层的合成。） */
  animation: stage-in 0.3s var(--ease-out-expo) backwards;
}
@keyframes stage-in {
  from {
    opacity: 0;
  }
}

/* ------------------------------------------------------------------ tabs */
.tabs {
  position: relative;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 0;
  align-self: center;
  padding: 3px;
  margin-bottom: 1.25rem;
  background: var(--glass-bg);
  border: 1px solid var(--glass-border);
  border-radius: 11px;
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-edge-soft);
}
/* 药丸宽度 = 一列宽度，所以 translateX(100%) 恰好落进第二格 */
.pill {
  position: absolute;
  top: 3px;
  bottom: 3px;
  left: 3px;
  width: calc((100% - 6px) / 2);
  border-radius: 8px;
  background: var(--glass-bg-strong);
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.1),
    0 3px 12px rgb(0 0 0 / 0.22);
  transition: transform var(--t-base) var(--ease-morph);
}
.tabs button {
  position: relative;
  padding: 0.35rem 1.5rem;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--ink-dim);
  font-size: 0.9rem;
  cursor: pointer;
  transition: color var(--t-base) var(--ease-out-expo);
}
.tabs button:hover {
  color: var(--ink-soft);
}
.tabs button.active {
  color: var(--ink);
}

/* ---------------------------------------------------------------- banner */
.banner {
  position: relative;
  display: flex;
  align-items: center;
  gap: 0.7rem;
  margin: 0 0 1.25rem;
  padding: 0.7rem 0.95rem 0.7rem 1.05rem;
  overflow: hidden;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius);
  background: var(--glass-bg);
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-edge), var(--glass-shadow);
  color: var(--ink-soft);
  font-size: 0.82rem;
  line-height: 1.5;
}
.banner-line {
  position: absolute;
  left: 0;
  top: 0;
  bottom: 0;
  width: 3px;
  background: var(--accent);
}
.banner-text {
  min-width: 0;
}
.banner-text b {
  color: var(--accent);
  font-weight: 600;
}
.dismiss {
  flex: none;
  margin-left: auto;
  padding: 0.2rem 0.6rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  background: transparent;
  color: var(--accent);
  cursor: pointer;
  font-size: 0.75rem;
  transition:
    background var(--t-base) var(--ease-out-expo),
    border-color var(--t-base) var(--ease-out-expo);
}
.dismiss:hover {
  background: var(--glass-bg-strong);
  border-color: var(--glass-border-strong);
}
.banner-enter-active {
  transition: opacity var(--t-base) var(--ease-out-expo), transform var(--t-base) var(--ease-morph);
}
.banner-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}
.banner-enter-from,
.banner-leave-to {
  opacity: 0;
  transform: translateY(-10px) scale(0.985);
}

/* ------------------------------------------------------------- 页面切换 */
/* 3D 翻页 + 模糊（参考集 Flip 的整页版）：
   出场页向左后方倒下并糊掉，进场页从右前方立起 —— perspective 放在
   .stage 上，这里只负责 rotateY / blur / opacity。blur 是过渡期瞬态，
   只存在于 0.3s 的切换窗口里，不构成常驻的大元素滤镜开销。 */
.flip-enter-active {
  transition:
    transform 0.5s var(--ease-out-expo),
    opacity 0.42s ease,
    filter 0.42s ease;
}
.flip-leave-active {
  transition:
    transform 0.28s ease-in,
    opacity 0.26s ease-in,
    filter 0.26s ease-in;
}
.flip-enter-from {
  opacity: 0;
  transform: translateX(52px) rotateY(9deg) scale(0.975);
  filter: blur(10px);
}
.flip-leave-to {
  opacity: 0;
  transform: translateX(-44px) rotateY(-8deg) scale(0.98);
  filter: blur(8px);
}

/* --------------------------------------------------------- 逐秒补间动画 */
/* 逐位翻页的过渡定义在 .clock 附近（.digit-enter/leave）—— 那里和槽宽、
   字号放在一起，改字号时能一眼看到两者的耦合关系。 */

/* ------------------------------------------------------------------ 计时页 */
.timer {
  position: relative; /* 侧栏状态栏与专注提示的定位基准 */
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 1.4rem;
  /* 表盘尺寸随窗口高度伸缩：350px 是除表盘外全部纵向 chrome 的实测值
     （标题栏 38 + stage 内边距 + tabs + 三行控件 + 提示 Tabs）。
     最小高度 560 的窗口仍能拿到 230px 的保底表盘，底部时长档不被裁切。
     第 24 轮：+26 —— 底部信息状态栏占掉的高度。 */
  --dial-size: clamp(230px, calc(100vh - 376px), 330px);
}

.readout {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.4rem;
  width: 100%;
}
/* 数字窗：盘心的一块玻璃胶囊。指针从它背后穿过 —— 数字不再与冒号、
   针尖直接相撞，"碰撞"变成有意的层次（毛玻璃把指针糊在后面）。 */
.clock-window {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.28rem;
  padding: 0.4rem 1.3rem 0.5rem;
  border-radius: 999px;
  background:
    linear-gradient(165deg, rgb(255 255 255 / 0.05), transparent 55%),
    var(--glass-bg-deep); /* 主题感知：晨雾下是浅玻璃，不再是发闷的深色胶囊 */
  border: 1px solid rgb(255 255 255 / 0.07);
  /* 第 19 轮：accent 环境色边缘光 —— 玻璃吸收状态色（专注绿/休息蓝紫），
     一圈 1px 微光实时跟随 --accent 插值，盘面像从内部透出光。 */
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.07),
    0 0 0 1px color-mix(in srgb, var(--accent) 9%, transparent),
    0 4px 18px rgb(0 0 0 / 0.28);
  backdrop-filter: blur(9px) saturate(1.25);
}
.clock {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 1.25em;
  margin: 0;
  /* 字号跟表盘走（0.17 倍）：320px 盘 ≈ 54px，最小 260px 盘 ≈ 44px */
  font-size: calc(var(--dial-size, 320px) * 0.17);
  /* 字体：优先 Segoe UI Variable（Win11 原生 UI 字体，数字更挺拔、笔画对比更明确），
     退回 Segoe UI。tabular-nums 与 "tnum" 特性双保险 —— 槽宽已经钉死了位置，
     这一步是让字形本身的视觉重心也保持一致。 */
  font-family: "Segoe UI Variable Display", "Segoe UI", system-ui, sans-serif;
  font-variant-numeric: tabular-nums;
  font-feature-settings: "tnum" 1;
  font-weight: 200;
  /* 关掉亚像素位移取整带来的字距抖动，Windows 上尤其明显 */
  text-rendering: geometricPrecision;
  color: var(--ink-soft);
  transition: color 0.4s ease, font-size 0.35s var(--ease-out-expo);
}
/* 超过 1 小时：7 字符长格式，字号降档避免溢出数字窗 */
.clock.long {
  font-size: calc(var(--dial-size, 320px) * 0.142);
}
/* 定宽数字槽。
   0.66em 是量出来的：Segoe UI 的数字 advance 约 0.5em，余 0.16em 当字距 ——
   既不挤在一起，读数也不显松散。关键是：**任何一位的位置都与它是什么数字无关**，
   于是 `01:11` 与 `01:20` 逐位对齐，换值只翻页、不位移。 */
.cell {
  position: relative;
  display: inline-grid;
  place-items: center;
  width: 0.66em;
  height: 1em;
  overflow: hidden; /* 翻页窗口：滚出去的那个数字在这里被裁掉 */
  line-height: 1;
}
.cell.sep {
  width: 0.34em;
  /* 冒号降为低透明度分隔符，运行时缓慢呼吸 —— 不再与指针抢视觉重心 */
  opacity: 0.4;
  transition: opacity 0.4s ease;
}
.cell .d {
  grid-area: 1 / 1;
  line-height: 1;
}
.running .clock .cell.sep {
  animation: sep-breathe 2.4s ease-in-out infinite;
}
@keyframes sep-breathe {
  50% {
    opacity: 0.14;
  }
}
/* 翻页：新数字自下而上顶入，旧数字继续向上滚出。
   方向恒定朝上 —— 不做「9 → 0 倒着滚回去」，读起来才是"数字在往前走"。 */
.digit-enter-active {
  transition: transform 0.4s var(--ease-out-expo), opacity 0.4s var(--ease-out-expo);
}
.digit-leave-active {
  transition: transform 0.3s ease-in, opacity 0.3s ease-in;
}
.digit-enter-from {
  transform: translateY(0.92em);
  opacity: 0;
}
.digit-leave-to {
  transform: translateY(-0.92em);
  opacity: 0;
}
.running .clock {
  /* 第 23 轮：数字也主题化 —— 深色主题跟状态色，浅色主题（dawn/paper）
     在 glass.css 覆盖 --clock-running 为深色（绿字浅底对比同指针一样不够） */
  color: var(--clock-running, var(--accent));
}
.state {
  display: flex;
  align-items: center;
  gap: 0.45rem;
  margin: 0;
  color: var(--ink-dim);
  font-size: 0.72rem;
  letter-spacing: 0.25em;
  text-transform: uppercase;
}
.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--accent);
  transition: background 0.4s ease;
}
.running .dot {
  animation: blink 2s ease-in-out infinite;
}
@keyframes blink {
  50% {
    opacity: 0.35;
  }
}

/* ------------------------------------------------------------------ 按钮 */
.controls {
  display: flex;
  gap: 0.6rem;
  justify-content: center;
}
.controls button {
  min-width: 6.4rem;
  padding: 0.55rem 1.2rem;
  border: 1px solid rgb(255 255 255 / 0.12);
  border-radius: var(--radius);
  /* liquid-glass 透明质感：低模糊高饱和 + 上缘内侧折射暗带（--lg-shadow），
     质感来自 shadow 层次而不是把背景糊掉 */
  background: var(--lg-bg);
  backdrop-filter: var(--lg-filter);
  box-shadow: var(--lg-shadow);
  color: var(--ink-soft);
  font: inherit;
  font-size: 0.92rem;
  cursor: pointer;
  transition:
    transform var(--t-base) var(--ease-out-back),
    background var(--t-base) var(--ease-out-expo),
    border-color var(--t-base) var(--ease-out-expo),
    color var(--t-base) var(--ease-out-expo),
    box-shadow var(--t-base) var(--ease-out-expo),
    opacity 0.2s ease;
}
.controls button:hover:not(:disabled) {
  border-color: rgb(255 255 255 / 0.24);
  background: var(--lg-bg-hover);
  transform: translateY(-1px);
}
.controls button:active:not(:disabled) {
  transform: scale(0.97);
}
.controls button:disabled {
  opacity: 0.4;
  cursor: default;
}

/* 「当前该按的那个」：强调色在开始/暂停之间转移。
   两边都有 background/border/color 的过渡，于是在切换的那 0.3s 里，
   强调色像是从一颗按钮流进了另一颗。 */
.controls .action.live {
  background: color-mix(in srgb, var(--accent) 88%, transparent);
  border-color: color-mix(in srgb, var(--accent) 88%, transparent);
  color: #0b0e13;
  font-weight: 600;
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.25),
    0 6px 20px color-mix(in srgb, var(--accent) 26%, transparent);
}
.controls .action.live:hover:not(:disabled) {
  background: color-mix(in srgb, var(--accent) 78%, white);
  border-color: color-mix(in srgb, var(--accent) 78%, white);
}

/* 开始按钮（liquid）单独走透明液态玻璃：强调色退到描边与文字上，
   按钮本体是透明的玻璃——三粒气泡在透明底上更有"液体"感 */
.controls .action.liquid.live {
  background: var(--lg-bg);
  border-color: color-mix(in srgb, var(--accent) 45%, transparent);
  color: var(--accent);
  box-shadow:
    var(--lg-shadow),
    0 0 18px color-mix(in srgb, var(--accent) 16%, transparent);
}
.controls .action.liquid.live:hover:not(:disabled) {
  background: var(--lg-bg-hover);
  border-color: color-mix(in srgb, var(--accent) 70%, transparent);
}

/* 文案形态融合：三层叠在同一网格单元，按钮宽度恒等于最宽的那层，
   切换时旧层失焦缩小淡出、新层从下方浮起，看起来是"融"进另一个形态 */
.labels {
  display: grid;
  place-items: center;
}
.labels > span {
  grid-area: 1 / 1;
  opacity: 0;
  transform: translateY(7px) scale(0.86);
  filter: blur(3px);
  transition:
    opacity var(--t-base) var(--ease-out-expo),
    transform var(--t-base) var(--ease-morph),
    filter var(--t-base) var(--ease-out-expo);
}
.labels > span.show {
  opacity: 1;
  transform: none;
  filter: none;
}

.controls .ghost {
  border-color: transparent;
  background: transparent;
  box-shadow: none;
  color: var(--ink-dim);
}
.controls .ghost:hover:not(:disabled) {
  color: #ff8a8a;
  border-color: color-mix(in srgb, #ff8a8a 35%, transparent);
  background: color-mix(in srgb, #ff8a8a 10%, transparent);
}

/* ------------------------------------------------------------------ 档位 */
.presets {
  display: flex;
  gap: 0.45rem;
  justify-content: center;
  align-items: center;
  flex-wrap: wrap;
}
.presets button {
  padding: 0.3rem 0.95rem;
  border: 1px solid rgb(255 255 255 / 0.11);
  border-radius: 999px;
  background: var(--lg-bg);
  backdrop-filter: var(--lg-filter);
  box-shadow: var(--lg-shadow);
  color: var(--ink-dim);
  font: inherit;
  font-size: 0.8rem;
  cursor: pointer;
  transition:
    color var(--t-base) var(--ease-out-expo),
    background var(--t-base) var(--ease-out-expo),
    border-color var(--t-base) var(--ease-out-expo),
    box-shadow var(--t-base) var(--ease-out-expo),
    transform var(--t-base) var(--ease-out-back);
}
.presets button:hover:not(:disabled) {
  color: var(--ink-soft);
  border-color: rgb(255 255 255 / 0.24);
  background: var(--lg-bg-hover);
  transform: translateY(-1px);
}
.presets button:active:not(:disabled) {
  transform: scale(0.96);
}
.presets button.active {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 15%, transparent);
  color: var(--accent);
  box-shadow: 0 0 14px color-mix(in srgb, var(--accent) 22%, transparent);
}
.presets button:disabled {
  opacity: 0.45;
  cursor: default;
}

/* 番茄钟开关：用专属暖红描边区别于普通档位，激活时常亮 */
.presets .pomo-toggle {
  border-color: color-mix(in srgb, #ff9c8f 35%, transparent);
  color: #c9736a;
}
.presets .pomo-toggle.active {
  border-color: #c9736a;
  background: color-mix(in srgb, #c9736a 18%, transparent);
  color: #ff9c8f;
  box-shadow: 0 0 14px rgb(201 115 106 / 0.28);
}

/* ------------------------------------------------------------ 科目标签行 */
/* ------------------------------------------------------ 自定义时长输入 */
.custom {
  display: flex;
  align-items: center;
  gap: 0.3rem;
  padding: 0.18rem 0.35rem 0.18rem 0.7rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  background: var(--glass-bg);
  backdrop-filter: var(--glass-blur);
  transition:
    border-color var(--t-base) var(--ease-out-expo),
    background var(--t-base) var(--ease-out-expo),
    box-shadow var(--t-base) var(--ease-out-expo);
}
.custom.active {
  border-color: var(--accent);
}
.custom.active .unit {
  color: var(--accent);
}
/* 聚焦辉光：输入时给一圈柔光，明确"这里正在编辑" */
.custom:focus-within {
  border-color: var(--accent);
  background: var(--glass-bg-strong);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
}
/* 设定成功：整组脉冲一次（back 曲线，弹性回位） */
.custom.pulse {
  animation: pulse 0.55s var(--ease-out-back);
}
@keyframes pulse {
  40% {
    transform: scale(1.06);
  }
}
.custom input {
  /* 宽度由内联 style 按位数以 em 算好，这里只兜上下界。
     ★ padding 必须显式归零：浏览器给 input 的默认内边距约 1px 2px，
       在只有一两个字符宽的字段里，那 4px 足以把 "30" 裁成半个数字。 */
  min-width: 1.5em;
  max-width: 3em;
  padding: 0;
  border: none;
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 0.8rem;
  text-align: center;
  font-variant-numeric: tabular-nums;
  font-feature-settings: "tnum" 1;
  outline: none;
  overflow: hidden;
  transition: width var(--t-base) var(--ease-out-expo);
}
/* 隐藏 number 输入的上下箭头，保持胶囊造型 */
.custom input::-webkit-outer-spin-button,
.custom input::-webkit-inner-spin-button {
  -webkit-appearance: none;
  margin: 0;
}
.custom input:disabled {
  opacity: 0.45;
}
.custom .unit {
  font-size: 0.75rem;
  color: var(--ink-dim);
  transition: color var(--t-base) ease;
}
.custom button {
  /* 文案在「设定」与「✓」之间切换，两者宽度差得远 —— 钉一个最小宽度，
     按钮不会在点击瞬间缩一下（原来的宽度恰好等于"设定"两字+内边距，没有余量） */
  min-width: 4.2em;
  padding: 0.22rem 0.7rem;
  border: none;
  border-radius: 999px;
  background: var(--glass-bg-strong);
  color: var(--ink-soft);
  font: inherit;
  font-size: 0.75rem;
  cursor: pointer;
  transition:
    background var(--t-base) var(--ease-out-expo),
    color var(--t-base) ease,
    transform var(--t-base) var(--ease-out-back);
}
.custom button:hover:not(:disabled) {
  background: var(--accent);
  color: #0b0e13;
  transform: scale(1.05);
}
.custom button:active:not(:disabled) {
  transform: scale(0.95);
}
.custom button.done {
  background: var(--accent);
  color: #0b0e13;
}
.custom button:disabled {
  opacity: 0.45;
  cursor: default;
}

/* -------------------------------------------------- 专注提示（Typewriter） */
/* 挂在计时页底部：开始专注时从窗口下缘浮现一句提示，不与表盘/控制区抢视觉。
   默认悬在 tips Tabs 上方（约两行高）；矮窗口 tips 隐藏时贴回底缘。 */
.focus-hint {
  position: absolute;
  bottom: 3.4rem;
  left: 0;
  right: 0;
  display: flex;
  justify-content: center;
  pointer-events: none;
}
@media (max-height: 660px) {
  .focus-hint {
    bottom: 0.6rem;
  }
}

/* -------------------------------------------------- 侧栏状态栏（Spotlight） */
/* 绝对定位挂在计时页左侧：不挤占表盘的居中布局，窄窗口整体隐藏。 */
.rail {
  position: absolute;
  left: 0;
  top: 50%;
  transform: translateY(-50%);
  display: flex;
  flex-direction: column;
  gap: 0.6rem;
  width: 150px;
}
@media (max-width: 899px) {
  .rail {
    display: none;
  }
}
.spot {
  position: relative;
  overflow: hidden;
  padding: 0.7rem 0.85rem 0.75rem;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius);
  background: var(--glass-bg);
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-edge-soft);
  transition: transform var(--t-base) var(--ease-out-back);
}
.spot:hover {
  transform: translateY(-2px);
}
/* 聚光灯：radial 圆心贴着鼠标（--sx/--sy 由脚本写入），悬停才浮现 */
.spot-light {
  position: absolute;
  inset: 0;
  pointer-events: none;
  background: radial-gradient(
    150px circle at var(--sx, 50%) var(--sy, 50%),
    color-mix(in srgb, var(--accent) 16%, transparent),
    transparent 65%
  );
  opacity: 0;
  transition: opacity 0.35s ease;
}
.spot:hover .spot-light {
  opacity: 1;
}
.spot-label {
  margin: 0;
  font-size: 0.62rem;
  letter-spacing: 0.2em;
  text-transform: uppercase;
  color: var(--ink-dim);
}
.spot-value {
  margin: 0.18rem 0 0;
  font-size: 1.18rem;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  color: var(--ink);
}
.spot-sub {
  margin: 0.12rem 0 0;
  font-size: 0.66rem;
  color: var(--ink-faint);
}

/* ---------------------------------------------------- Liquid 开始按钮 */
.action.liquid {
  position: relative;
  overflow: hidden;
}
.action.liquid .labels {
  position: relative;
  z-index: 1;
}
/* 霓虹文字（参考集 Neon）：多层光晕 + 呼吸明暗。只挂在可点击的
   液态按钮上 —— 运行后按钮禁用转暗，霓虹随之熄灭，"亮着的字"始终指向
   下一个可执行的动作。霓虹基色 = accent 提亮 55% 白：空闲态的灰也能发出
   可读的银雾光晕（纯 accent 灰的霓虹肉眼看不出"亮"），暂停琥珀 / 完成蓝
   自动跟随。hover 时文字本体点亮成纯白（参考 .liquid-btn:hover），
   光晕加宽一档，像霓虹管被通上了满电流。 */
.action.liquid:not(:disabled) .labels {
  --neon: color-mix(in srgb, var(--accent) 55%, white);
  color: var(--neon);
  animation: neon-breath 2.2s ease-in-out infinite alternate;
}
.action.liquid:not(:disabled):hover .labels {
  color: #fff;
  animation-name: neon-breath-hot;
}
@keyframes neon-breath {
  from {
    text-shadow:
      0 0 6px var(--neon),
      0 0 12px color-mix(in srgb, var(--neon) 60%, transparent),
      0 0 24px color-mix(in srgb, var(--neon) 32%, transparent);
  }
  to {
    text-shadow:
      0 0 8px var(--neon),
      0 0 18px color-mix(in srgb, var(--neon) 78%, transparent),
      0 0 40px color-mix(in srgb, var(--neon) 44%, transparent);
  }
}
@keyframes neon-breath-hot {
  from {
    text-shadow:
      0 0 8px rgb(255 255 255 / 0.7),
      0 0 16px var(--neon),
      0 0 36px color-mix(in srgb, var(--neon) 50%, transparent);
  }
  to {
    text-shadow:
      0 0 10px rgb(255 255 255 / 0.9),
      0 0 24px var(--neon),
      0 0 56px color-mix(in srgb, var(--neon) 62%, transparent);
  }
}
.action.liquid .blob {
  position: absolute;
  border-radius: 50%;
  background: rgb(255 255 255 / 0.24);
  filter: blur(8px); /* 小元素 + 短半径：不触碰"大元素禁 blur"红线 */
  pointer-events: none;
  animation: blob-float 4s ease-in-out infinite;
}
.action.liquid .blob:nth-of-type(1) {
  width: 34px;
  height: 34px;
  top: -12px;
  left: 14%;
}
.action.liquid .blob:nth-of-type(2) {
  width: 22px;
  height: 22px;
  bottom: -9px;
  right: 18%;
  animation-delay: -1.2s;
  animation-duration: 4.6s;
}
.action.liquid .blob:nth-of-type(3) {
  width: 27px;
  height: 27px;
  top: 38%;
  left: 58%;
  animation-delay: -2.4s;
  animation-duration: 3.6s;
}
@keyframes blob-float {
  0%,
  100% {
    transform: translate(0, 0) scale(1);
  }
  33% {
    transform: translate(10px, -7px) scale(1.18);
  }
  66% {
    transform: translate(-8px, 6px) scale(0.88);
  }
}
/* 按下运行后按钮禁用，气泡随之冻住 —— "液体凝固"暗示不可点 */
.action.liquid:disabled .blob {
  animation-play-state: paused;
  opacity: 0.55;
}

/* ---------------------------------------------------- 冥想模式 */
.meditation-layer {
  position: fixed;
  inset: 0;
  z-index: var(--z-smoke);
  pointer-events: none;
}
/* 呼吸圆环：挂在表盘根元素上的第二圈边框，4s 周期缩放 + 光晕 */
.meditating .dial {
  border-color: color-mix(in srgb, var(--accent) 42%, transparent);
}
.meditating .dial::after {
  content: "";
  position: absolute;
  inset: -12px;
  border-radius: 50%;
  border: 2px solid color-mix(in srgb, var(--accent) 52%, transparent);
  box-shadow:
    0 0 34px color-mix(in srgb, var(--accent) 24%, transparent),
    inset 0 0 26px color-mix(in srgb, var(--accent) 10%, transparent);
  pointer-events: none;
  animation: ring-breathe 4s var(--ease-in-out-soft) infinite;
}
@keyframes ring-breathe {
  0%,
  100% {
    transform: scale(0.97);
    opacity: 0.5;
  }
  50% {
    transform: scale(1.04);
    opacity: 1;
  }
}

/* ---------------------------------------------------- 氛围控制（冥想/主题） */
.aux {
  display: inline-flex;
  align-items: center;
  gap: 0.6rem;
  margin-left: 0.3rem;
  padding-left: 0.85rem;
  border-left: 1px solid var(--glass-border);
}
/* 冥想档位：借 presets 按钮的壳，紫色描边区别于番茄的暖红 */
.presets .meditate {
  border-color: color-mix(in srgb, #b58cff 35%, transparent);
  color: #9d7fe0;
}
.presets .meditate.active {
  border-color: #b58cff;
  background: color-mix(in srgb, #b58cff 16%, transparent);
  color: #cdb4ff;
  box-shadow: 0 0 14px rgb(181 140 255 / 0.28);
}
/* ---------------------------------------------------- 使用提示（Tabs） */
.tips {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.45rem;
  align-self: center;
  width: min(560px, 100%);
}
.tips-nav {
  position: relative;
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  padding: 3px;
  border: 1px solid var(--glass-border);
  border-radius: 12px;
  background: var(--glass-bg);
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-edge-soft);
}
/* 滑动指示器：宽度恒等于一列，translateX(100%) 恰好挪到下一格 */
.tips-indicator {
  position: absolute;
  top: 3px;
  bottom: 3px;
  left: 3px;
  width: calc((100% - 6px) / 3);
  border-radius: 9px;
  background: var(--glass-bg-strong);
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.1),
    0 3px 12px rgb(0 0 0 / 0.22);
  transition: transform var(--t-base) var(--ease-morph);
}
.tips-nav button {
  position: relative;
  z-index: 1;
  padding: 0.28rem 1rem;
  border: none;
  border-radius: 9px;
  background: transparent;
  color: var(--ink-dim);
  font: inherit;
  font-size: 0.78rem;
  cursor: pointer;
  white-space: nowrap;
  transition: color var(--t-base) var(--ease-out-expo);
}
.tips-nav button:hover {
  color: var(--ink-soft);
}
.tips-nav button.active {
  color: var(--ink);
}
.tips-body {
  margin: 0;
  min-height: 2.4em;
  max-width: 560px;
  text-align: center;
  font-size: 0.78rem;
  line-height: 1.65;
  color: var(--ink-dim);
}
.tipslide-enter-active,
.tipslide-leave-active {
  transition:
    opacity 0.22s ease,
    transform 0.22s ease;
}
.tipslide-enter-from {
  opacity: 0;
  transform: translateY(6px);
}
.tipslide-leave-to {
  opacity: 0;
  transform: translateY(-6px);
}

/* ---------------------------------------------------- 冥想粒子层淡入淡出 */
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.6s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

/* 动效偏好减弱时，全部退化为直接呈现 */
@media (prefers-reduced-motion: reduce) {
  .running .dot,
  .custom.pulse,
  .action.liquid .blob,
  .action.liquid .labels,
  .meditating .dial::after {
    animation: none;
  }
  .running .clock .sep {
    animation: none;
  }
  .banner-enter-active,
  .banner-leave-active,
  .flip-enter-active,
  .flip-leave-active,
  .tipslide-enter-active,
  .tipslide-leave-active,
  .fade-enter-active,
  .fade-leave-active,
  .digit-enter-active,
  .digit-leave-active,
  .pill,
  .tips-indicator,
  .labels > span,
  .controls button,
  .presets button,
  .clock {
    transition: none;
  }
}

/* --------------------------------------------------- 矮窗口：压缩纵向 chrome
   最小高度 560 时若不收紧间距，底部时长档会被裁出视口（需求：所有组件自适应）。
   表盘本体已经通过 --dial-size 随高度缩放，这里再把固定开销压掉一截。 */
@media (max-height: 660px) {
  .stage {
    padding: 0.75rem 1.25rem 1.1rem;
  }
  .tabs {
    margin-bottom: 0.8rem;
  }
  .timer {
    gap: 0.85rem;
  }
  /* 矮窗口砍掉提示区，把高度留给表盘 */
  .tips {
    display: none;
  }
}

/* ================================================================ 组件控制台
   右下角一枚玻璃圆钮 + 上方弹出的小面板。z-index 高于内容低于 Toast。 */
.ui-console {
  position: fixed;
  right: 18px;
  bottom: 18px;
  z-index: 60;
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 10px;
}
.ui-fab {
  display: grid;
  place-items: center;
  width: 40px;
  height: 40px;
  border: 1px solid var(--glass-border);
  border-radius: 50%;
  background: var(--glass-bg);
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-edge), 0 8px 22px rgb(0 0 0 / 0.3);
  cursor: pointer;
  transition:
    border-color var(--t-base) var(--ease-out-expo),
    transform var(--t-base) var(--ease-out-back),
    box-shadow var(--t-base) ease;
}
.ui-fab:hover {
  border-color: var(--glass-border-strong);
  transform: translateY(-2px);
}
.ui-fab-icon,
.ui-fab-icon::before {
  display: block;
  width: 14px;
  height: 14px;
  border: 1.5px solid var(--ink-dim);
  transition: border-color var(--t-base) ease;
}
.ui-fab-icon {
  border-radius: 3px;
  position: relative;
}
.ui-fab-icon::before {
  content: "";
  position: absolute;
  inset: 3px;
  border-radius: 1.5px;
  opacity: 0.55;
}
.ui-fab:hover .ui-fab-icon,
.ui-fab:hover .ui-fab-icon::before {
  border-color: var(--ui-accent);
}
.ui-panel {
  min-width: 168px;
  padding: 0.7rem 0.85rem 0.75rem;
  border: 1px solid var(--glass-border);
  border-radius: 14px;
  background: var(--glass-bg-deep);
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-edge), 0 16px 40px rgb(0 0 0 / 0.38);
}
.ui-panel-title {
  margin: 0 0 0.5rem;
  font-size: 0.68rem;
  letter-spacing: 0.08em;
  color: var(--ink-dim);
}
.ui-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 1.2rem;
  padding: 0.3rem 0;
  font-size: 0.78rem;
  color: var(--ink-soft);
  cursor: pointer;
}
.ui-row .switch {
  flex: none;
  width: 30px;
  height: 17px;
  border-radius: 999px;
  background: rgb(128 128 128 / 0.32);
  position: relative;
  transition: background var(--t-base) ease;
}
.ui-row .switch::after {
  content: "";
  position: absolute;
  top: 2px;
  left: 2px;
  width: 13px;
  height: 13px;
  border-radius: 50%;
  background: rgb(255 255 255 / 0.9);
  transition: transform var(--t-base) var(--ease-out-back);
}
.ui-row .switch.on {
  background: var(--ui-accent);
}
.ui-row .switch.on::after {
  transform: translateX(13px);
}
.immersive-enter,
.immersive-exit {
  margin-top: 0.55rem;
  width: 100%;
  padding: 0.36rem 0;
  border: 1px solid color-mix(in srgb, var(--ui-accent) 45%, transparent);
  border-radius: 9px;
  background: color-mix(in srgb, var(--ui-accent) 14%, transparent);
  color: var(--ui-accent);
  font: inherit;
  font-size: 0.76rem;
  cursor: pointer;
  transition:
    background var(--t-base) ease,
    border-color var(--t-base) ease,
    color var(--t-base) ease;
}
.immersive-enter:hover {
  background: color-mix(in srgb, var(--ui-accent) 26%, transparent);
}
/* 沉浸退出幽灵钮：右上角，平时 8% 透明度几乎隐形，hover 才浮现 */
.immersive-exit {
  position: fixed;
  top: 12px;
  right: 14px;
  z-index: 60;
  width: 30px;
  height: 30px;
  margin: 0;
  padding: 0;
  border: none;
  border-radius: 50%;
  background: rgb(128 128 128 / 0.14);
  color: var(--ink-dim);
  font-size: 1rem;
  line-height: 1;
  opacity: 0.16;
  transition: opacity var(--t-base) ease, background var(--t-base) ease;
}
.immersive-exit:hover {
  opacity: 0.85;
  background: rgb(128 128 128 / 0.3);
}
</style>
