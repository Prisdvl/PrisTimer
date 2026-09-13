<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
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
import ParticleField from "./components/ParticleField.vue";
import SmokeField from "./components/SmokeField.vue";
import StatsView from "./components/StatsView.vue";
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

// 背景主题与底部主题菜单：纯逻辑在 composables/useTheme.ts
const {
  THEMES,
  theme,
  currentThemeLabel,
  currentThemeSwatch,
  themeMenuOpen,
  pickTheme,
  onGlobalPointerDown,
  onGlobalKeydown,
  syncTheme,
} = useTheme();

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

// 番茄钟状态/设置面板/循环序列拖拽：纯逻辑在 composables/usePomodoro.ts
const {
  pomodoro,
  pomoConfig,
  pomoSettingsOpen,
  pomoFeedback,
  pomoDraft,
  syncDraft,
  applyPomoConfig,
  seqLocal,
  dragFrom,
  seqSnapping,
  onDropSeq,
  togglePomodoro,
  totalDots,
  litDots,
  phaseAccent,
  SEQ_LABEL,
} = usePomodoro();

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

// 科目标签集合与选中态：纯逻辑在 composables/useTags.ts
const {
  tags,
  selectedTag,
  tagInput,
  editingTag,
  editingText,
  applyTag,
  submitTag,
  startTagEdit,
  cancelTagEdit,
  commitTagEdit,
  removeTag,
} = useTags();

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
  // 主题菜单的外点关闭 / Escape 关闭（第 21 轮）
  document.addEventListener("pointerdown", onGlobalPointerDown);
  document.addEventListener("keydown", onGlobalKeydown);
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

onUnmounted(() => {
  document.removeEventListener("pointerdown", onGlobalPointerDown);
  document.removeEventListener("keydown", onGlobalKeydown);
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
    <SmokeField :paused="mini" :theme="theme" :active="isRunning" />
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
    <!-- 启动动画：Scramble 解码标题 + 粒子背景，~1.5s 后摘掉 -->
    <IntroSplash v-if="!introDone && !mini" @done="introDone = true" />

    <div class="shell" :class="{ mini, 'morph-out': morphOut }">
      <TitleBar v-if="!mini" @close="onWindowClose" @mini="toggleMini" />

      <!-- 迷你模式：整窗缩成一枚贴边置顶的"组件"，只留时间与开始/暂停。
           整面可拖拽（data-tauri-drag-region），按钮不拖拽只响应点击；
           底部细线是倒计时进度（与主表盘的细弧同源）。 -->
      <div v-if="mini" class="mini" data-tauri-drag-region>
        <span
          v-if="ARC_PROGRESS !== null"
          class="mini-progress"
          :style="{ transform: `scaleX(${ARC_PROGRESS})` }"
        />
        <div class="mini-read" data-tauri-drag-region>
          <p class="mini-clock" :class="{ run: isRunning }">{{ display }}</p>
          <p class="mini-state">
            <i class="dot" />{{ miniLabel }}
            <span v-if="selectedTag" class="mini-tag">{{ selectedTag }}</span>
          </p>
        </div>
        <div class="mini-actions">
          <button
            class="mini-btn"
            :class="{ live: isRunning }"
            :title="isRunning ? '暂停' : '开始'"
            @click="isRunning ? timerApi.pause() : timerApi.start()"
          >
            <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
              <rect v-if="isRunning" x="2" y="1.5" width="3" height="9" rx="1" fill="currentColor" />
              <rect v-if="isRunning" x="7" y="1.5" width="3" height="9" rx="1" fill="currentColor" />
              <path v-else d="M3 1.6v8.8c0 .5.55.8.97.53l7-4.4a.62.62 0 0 0 0-1.06l-7-4.4A.62.62 0 0 0 3 1.6Z" fill="currentColor" />
            </svg>
          </button>
          <button class="mini-btn" title="还原窗口" @click="toggleMini">
            <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" fill="none">
              <path
                d="M1.2 4.4V1.2h3.2M10.8 4.4V1.2H7.6M1.2 7.6v3.2h3.2M10.8 7.6v3.2H7.6"
                stroke="currentColor"
                stroke-width="1.2"
                stroke-linecap="round"
              />
            </svg>
          </button>
          <button class="mini-btn" title="收进托盘（计时继续）" @click="closeToTray">
            <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" fill="none">
              <path
                d="M2 2.5h8M2 6h8M2 9.5h5"
                stroke="currentColor"
                stroke-width="1.2"
                stroke-linecap="round"
              />
            </svg>
          </button>
        </div>
      </div>

      <div v-else class="stage">
        <nav class="tabs">
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
            <div class="focus-hint" aria-live="polite">
              <TypewriterHint v-if="focusHint" :text="focusHint" @done="focusHint = null" />
            </div>

            <!-- 侧栏状态栏（Spotlight）：今日专注 + 番茄数。绝对定位不挤占表盘 -->
            <aside class="rail" aria-label="今日状态">
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

            <div class="controls">
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

            <!-- 科目标签行：选中的科目会落到下一条会话上，进统计与 CSV。
                 标签本身可维护 —— 双击重命名，悬停出现 × 删除，输入框回车新增。 -->
            <div class="tags">
              <div v-for="tag in tags" :key="tag" class="tag-slot">
                <input
                  v-if="editingTag === tag"
                  v-model="editingText"
                  class="tag-edit"
                  type="text"
                  maxlength="20"
                  @keyup.enter="commitTagEdit"
                  @keyup.esc="cancelTagEdit"
                  @blur="commitTagEdit"
                />
                <button
                  v-else
                  class="tag-chip"
                  :class="{ active: selectedTag === tag }"
                  :title="`${tag} · 双击重命名`"
                  @click="applyTag(selectedTag === tag ? null : tag)"
                  @dblclick="startTagEdit(tag)"
                >
                  <span class="chip-text">{{ tag }}</span>
                  <span class="chip-del" title="删除这个标签" @click.stop="removeTag(tag)">×</span>
                </button>
              </div>
              <input
                v-model="tagInput"
                class="tag-input"
                type="text"
                maxlength="20"
                placeholder="自定义"
                title="输入科目后按回车添加"
                @keyup.enter="submitTag"
              />
            </div>

            <!-- 番茄阶段条：开启后常驻，循环推进全在 Rust 侧，这里只读状态。
                 点击展开设置面板（齿轮），循环参数即改即存。 -->
            <Transition name="pomo">
              <div v-if="pomodoro?.enabled" class="pomo-wrap">
                <button
                  class="pomo-strip"
                  :aria-expanded="pomoSettingsOpen"
                  title="点击调整番茄时长"
                  @click="pomoSettingsOpen = !pomoSettingsOpen"
                >
                  <span class="pomo-label">{{ PHASE_LABEL[pomodoro.phase] }}</span>
                  <span class="pomo-dots">
                    <i v-for="n in totalDots" :key="n" :class="{ done: litDots >= n }" />
                  </span>
                  <svg class="pomo-gear" :class="{ open: pomoSettingsOpen }" viewBox="0 0 24 24" width="13" height="13">
                    <path
                      fill="currentColor"
                      d="M19.14 12.94a7.5 7.5 0 0 0 .06-.94 7.5 7.5 0 0 0-.06-.94l2.03-1.58a.5.5 0 0 0 .12-.64l-1.92-3.32a.5.5 0 0 0-.61-.22l-2.39.96a7.3 7.3 0 0 0-1.62-.94l-.36-2.54a.5.5 0 0 0-.5-.42h-3.84a.5.5 0 0 0-.5.42l-.36 2.54c-.59.24-1.13.56-1.62.94l-2.39-.96a.5.5 0 0 0-.61.22L2.65 8.84a.5.5 0 0 0 .12.64l2.03 1.58a7.5 7.5 0 0 0 0 1.88l-2.03 1.58a.5.5 0 0 0-.12.64l1.92 3.32c.13.23.4.32.61.22l2.39-.96c.49.38 1.03.7 1.62.94l.36 2.54c.04.24.25.42.5.42h3.84c.25 0 .46-.18.5-.42l.36-2.54a7.3 7.3 0 0 0 1.62-.94l2.39.96c.21.1.48.01.61-.22l1.92-3.32a.5.5 0 0 0-.12-.64l-2.03-1.58ZM12 15.5A3.5 3.5 0 1 1 12 8.5a3.5 3.5 0 0 1 0 7Z"
                    />
                  </svg>
                </button>

                <Transition name="pomo-settings">
                  <form v-if="pomoSettingsOpen" class="pomo-settings" @submit.prevent="applyPomoConfig">
                    <label>
                      <span>专注</span>
                      <input v-model.number="pomoDraft.focusMin" type="number" min="1" max="120" />
                    </label>
                    <label>
                      <span>短休</span>
                      <input v-model.number="pomoDraft.shortMin" type="number" min="1" max="120" />
                    </label>
                    <label>
                      <span>长休</span>
                      <input v-model.number="pomoDraft.longMin" type="number" min="1" max="120" />
                    </label>
                    <label>
                      <span>轮数</span>
                      <input v-model.number="pomoDraft.rounds" type="number" min="2" max="8" />
                    </label>
                    <button type="submit" class="pomo-apply">应用</button>
                    <span class="pomo-hint" :class="pomoFeedback">
                      {{ pomoFeedback === "saved" ? "已保存 ✓" : pomoFeedback === "error" ? "保存失败" : "分钟 / 长休前轮数" }}
                    </span>

                    <!-- 循环序列（Draggable）：拖「长休息」卡改它的位置 = 改轮数 -->
                    <div class="seq" :class="{ snapping: seqSnapping }">
                      <div
                        v-for="(card, i) in seqLocal"
                        :key="card.id"
                        class="seq-card"
                        :class="[card.kind, { dragging: dragFrom === i }]"
                        draggable="true"
                        title="拖动调整顺序"
                        @dragstart="dragFrom = i"
                        @dragover.prevent
                        @drop.prevent="onDropSeq(i)"
                        @dragend="dragFrom = -1"
                      >
                        <span class="seq-handle">⋮⋮</span>
                        <span class="seq-name">{{ SEQ_LABEL[card.kind] }}</span>
                        <span class="seq-min">{{ card.minutes }} 分</span>
                      </div>
                    </div>
                    <p class="seq-hint">拖动「长休息」卡片可改变它的位置（＝长休前的专注轮数）；专注与短休的交替由番茄节奏固定</p>
                  </form>
                </Transition>
              </div>
            </Transition>

            <div class="presets">
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
                <span class="theme-ctl" ref="themeCtlRoot">
                  <!-- 第 21 轮：主题键升级为底部菜单 —— 按钮显示当前主题
                       （色点 + 名称），点击向上弹出玻璃菜单，六主题带选中态 -->
                  <button
                    class="theme-btn"
                    :class="{ open: themeMenuOpen }"
                    :aria-expanded="themeMenuOpen"
                    title="切换主题"
                    @click="themeMenuOpen = !themeMenuOpen"
                  >
                    <i class="theme-dot" :style="{ background: currentThemeSwatch }" />
                    <span>{{ currentThemeLabel }}</span>
                    <svg class="chev" viewBox="0 0 10 6" aria-hidden="true">
                      <path d="M1 1l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
                    </svg>
                  </button>
                  <Transition name="theme-pop">
                    <span v-if="themeMenuOpen" class="theme-menu" role="radiogroup" aria-label="背景主题">
                      <button
                        v-for="t in THEMES"
                        :key="t.id"
                        class="theme-item"
                        :class="{ active: theme === t.id }"
                        role="radio"
                        :aria-checked="theme === t.id"
                        @click="pickTheme(t.id)"
                      >
                        <i class="sw" :style="{ background: t.swatch }" />
                        <span>{{ t.label }}</span>
                        <i v-if="theme === t.id" class="check" aria-hidden="true">✓</i>
                      </button>
                    </span>
                  </Transition>
                </span>
              </span>
            </div>

            <!-- 使用提示：Tabs 滑动指示器 + 内容淡入 -->
            <div class="tips">
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
.mini {
  position: fixed;
  inset: 0;
  z-index: var(--z-content);
  overflow: hidden;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
  padding: 0 0.65rem 0 0.9rem;
  /* 圆角跟 Rust 侧的 DWM 圆角（ROUNDSMALL）对齐；CSS 这层负责把内部的进度线、
     按钮底色裁进同一轮廓 —— 只靠 DWM 裁窗口、内部还画满直角，会露出尖角。 */
  border-radius: 8px;
  background:
    linear-gradient(165deg, rgb(255 255 255 / 0.055), transparent 55%),
    var(--glass-bg-strong);
  backdrop-filter: var(--glass-blur-lg);
  user-select: none;
  animation: mini-in 0.34s var(--ease-out-expo) backwards;
}
/* 进场：从略小、略透明处"贴"出来，与窗口自身由大到小的收缩连成一件事 */
@keyframes mini-in {
  from {
    opacity: 0;
    transform: scale(0.94);
  }
}
/* 倒计时进度细线：与主表盘的外圈细弧同源（ARC_PROGRESS），
   让"还剩多少"在余光里也能读到 */
.mini-progress {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 2px;
  background: var(--accent);
  opacity: 0.8;
  transform-origin: left center;
  transition: transform 0.4s var(--ease-out-quart);
  pointer-events: none;
}
.mini-read {
  min-width: 0;
}
.mini-clock {
  margin: 0;
  font-size: 1.72rem;
  line-height: 1.15;
  font-weight: 200;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.04em;
  color: var(--ink-soft);
  transition: color 0.4s ease;
}
.mini-clock.run {
  color: var(--accent);
}
.mini-state {
  display: flex;
  align-items: center;
  gap: 0.35rem;
  margin: 0;
  font-size: 0.6rem;
  letter-spacing: 0.22em;
  text-transform: uppercase;
  color: var(--ink-dim);
}
.mini-tag {
  padding: 0.05rem 0.4rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  font-size: 0.58rem;
  letter-spacing: 0.08em;
  color: var(--ink-soft);
  background: var(--glass-bg);
}
.mini-actions {
  display: flex;
  gap: 0.35rem;
  flex: none;
}
.mini-btn {
  display: grid;
  place-items: center;
  width: 30px;
  height: 30px;
  padding: 0;
  border: 1px solid rgb(255 255 255 / 0.14);
  border-radius: 9px;
  background: var(--lg-bg);
  box-shadow: var(--lg-shadow);
  color: var(--ink-soft);
  cursor: pointer;
  backdrop-filter: var(--lg-filter);
  transition:
    background var(--t-fast) ease,
    border-color var(--t-fast) ease,
    color var(--t-fast) ease,
    transform var(--t-fast) var(--ease-out-back);
}
.mini-btn:hover {
  background: var(--lg-bg-hover);
  border-color: rgb(255 255 255 / 0.26);
  color: var(--ink);
}
.mini-btn:active {
  transform: scale(0.94);
}
.mini-btn.live {
  border-color: color-mix(in srgb, var(--accent) 55%, transparent);
  background: color-mix(in srgb, var(--accent) 16%, transparent);
  color: var(--accent);
}

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
     最小高度 560 的窗口仍能拿到 230px 的保底表盘，底部时长档不被裁切。 */
  --dial-size: clamp(230px, calc(100vh - 350px), 330px);
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
  color: var(--accent);
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
.tags {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-wrap: wrap;
  gap: 0.45rem;
}
/* 每个标签占一个槽：输入态与展示态在同一位置切换，不会把整行挤动 */
.tag-slot {
  display: inline-flex;
}
.tag-chip {
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
  /* 右侧多留一点：删除键常驻占位（只是透明），悬停时出现不会引起位移 */
  padding: 0.22rem 0.4rem 0.22rem 0.85rem;
  max-width: 11rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  background: var(--glass-bg);
  backdrop-filter: var(--glass-blur);
  color: var(--ink-dim);
  font: inherit;
  font-size: 0.75rem;
  cursor: pointer;
  transition:
    color var(--t-base) var(--ease-out-expo),
    border-color var(--t-base) var(--ease-out-expo),
    background var(--t-base) var(--ease-out-expo),
    box-shadow var(--t-base) var(--ease-out-expo),
    transform var(--t-base) var(--ease-out-back);
}
.tag-chip:hover {
  color: var(--ink-soft);
  border-color: var(--glass-border-strong);
  transform: translateY(-1px);
}
.tag-chip:active {
  transform: scale(0.95);
}
.tag-chip.active {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 16%, transparent);
  color: var(--accent);
  box-shadow: 0 0 12px color-mix(in srgb, var(--accent) 26%, transparent);
}
/* 自定义标签可能很长（上限 20 字）：文字截断而不是把整行撑爆 */
.chip-text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* 删除键：平时透明占位，悬停整颗胶囊时浮现 */
.chip-del {
  flex: none;
  display: grid;
  place-items: center;
  width: 1.1em;
  height: 1.1em;
  border-radius: 50%;
  font-size: 1.1em;
  line-height: 1;
  opacity: 0;
  transform: scale(0.6);
  transition:
    opacity var(--t-fast) ease,
    transform var(--t-fast) var(--ease-out-back),
    background var(--t-fast) ease,
    color var(--t-fast) ease;
}
.tag-chip:hover .chip-del {
  opacity: 0.7;
  transform: none;
}
.chip-del:hover {
  opacity: 1;
  background: rgb(255 120 120 / 0.28);
  color: #ffd0d0;
}
/* 内联重命名的输入框：与胶囊同高同形，切换上去像是同一颗控件换了状态 */
.tag-edit {
  width: 7em;
  padding: 0.22rem 0.7rem;
  border: 1px solid var(--accent);
  border-radius: 999px;
  background: var(--glass-bg-strong);
  color: var(--ink);
  font: inherit;
  font-size: 0.75rem;
  text-align: center;
  outline: none;
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
}
/* ★ 宽度必须放得下 placeholder「自定义」三个汉字。
   原来是 5ch —— ch 是数字 "0" 的宽度（0.75rem 下约 6.7px），5ch ≈ 33px，
   再扣掉左右 padding 就只剩十几个像素，三个汉字（36px）根本显示不全。
   改用 em（1em = 一个汉字宽）来对齐意图。 */
.tag-input {
  width: 5em;
  padding: 0.22rem 0.5rem;
  border: 1px dashed rgb(255 255 255 / 0.18);
  border-radius: 999px;
  background: transparent;
  color: var(--ink-soft);
  font: inherit;
  font-size: 0.75rem;
  text-align: center;
  transition:
    width var(--t-base) var(--ease-out-expo),
    border-color var(--t-base) ease,
    color var(--t-base) ease,
    background var(--t-base) ease;
}
.tag-input::placeholder {
  color: #565d68;
}
.tag-input:focus {
  outline: none;
  border-style: solid;
  border-color: var(--accent);
  width: 9em;
  background: var(--glass-bg);
}

/* ------------------------------------------------------------ 番茄阶段条 */
.pomo-wrap {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.5rem;
  align-self: center;
}
.pomo-strip {
  display: flex;
  align-items: center;
  gap: 0.9rem;
  padding: 0.4rem 1.1rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  background: var(--glass-bg);
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-edge-soft);
  cursor: pointer;
  font: inherit;
  color: inherit;
  transition:
    border-color var(--t-base) var(--ease-out-expo),
    background var(--t-base) var(--ease-out-expo),
    transform var(--t-base) var(--ease-out-back);
}
.pomo-strip:hover {
  border-color: var(--glass-border-strong);
  background: var(--glass-bg-strong);
}
.pomo-strip:active {
  transform: scale(0.97);
}
.pomo-label {
  font-size: 0.75rem;
  letter-spacing: 0.22em;
  color: var(--ink-dim);
  text-transform: uppercase;
}
.pomo-dots {
  display: flex;
  gap: 0.35rem;
}
.pomo-dots i {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: rgb(255 255 255 / 0.12);
  transition:
    background 0.35s var(--ease-out-expo),
    box-shadow 0.35s var(--ease-out-expo),
    transform 0.35s var(--ease-out-back);
}
.pomo-dots i.done {
  background: var(--accent);
  box-shadow: 0 0 8px color-mix(in srgb, var(--accent) 60%, transparent);
  transform: scale(1.1);
}
.pomo-enter-active {
  transition: opacity var(--t-base) var(--ease-out-expo), transform var(--t-base) var(--ease-morph);
}
.pomo-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}
.pomo-enter-from,
.pomo-leave-to {
  opacity: 0;
  transform: translateY(8px) scale(0.97);
}
.pomo-gear {
  color: var(--ink-faint);
  transition: transform 0.45s var(--ease-out-back), color var(--t-base) ease;
}
.pomo-gear.open {
  transform: rotate(90deg);
  color: #9ca3af;
}

/* ------------------------------------------------------------ 番茄设置面板 */
.pomo-settings {
  display: flex;
  align-items: center;
  gap: 0.8rem;
  padding: 0.55rem 1rem;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  background: var(--glass-bg);
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-edge), var(--glass-shadow);
}
.pomo-settings label {
  display: flex;
  align-items: baseline;
  gap: 0.35rem;
  font-size: 0.72rem;
  color: var(--ink-dim);
}
.pomo-settings input {
  width: 3.2ch;
  padding: 0.15rem 0.3rem;
  border: 1px solid rgb(255 255 255 / 0.12);
  border-radius: 0.45rem;
  background: var(--glass-bg-deep);
  color: var(--ink);
  font: inherit;
  font-size: 0.8rem;
  text-align: center;
  transition: border-color var(--t-base) ease, box-shadow var(--t-base) ease;
}
.pomo-settings input:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 25%, transparent);
}
/* 隐藏 number 输入的上下箭头，视觉更干净 */
.pomo-settings input::-webkit-outer-spin-button,
.pomo-settings input::-webkit-inner-spin-button {
  -webkit-appearance: none;
  margin: 0;
}
.pomo-settings input[type="number"] {
  appearance: textfield;
  -moz-appearance: textfield;
}
.pomo-apply {
  padding: 0.28rem 0.85rem;
  border: 1px solid var(--glass-border-strong);
  border-radius: 999px;
  background: var(--glass-bg-strong);
  color: var(--ink-soft);
  font: inherit;
  font-size: 0.75rem;
  cursor: pointer;
  transition: background var(--t-base) var(--ease-out-expo), transform var(--t-base) var(--ease-out-expo);
}
.pomo-apply:hover {
  background: rgb(255 255 255 / 0.14);
}
.pomo-apply:active {
  transform: scale(0.95);
}
.pomo-hint {
  font-size: 0.68rem;
  color: var(--ink-faint);
  transition: color var(--t-base) ease;
}
.pomo-hint.saved {
  color: #6fe0a8;
}
.pomo-hint.error {
  color: #ff8f8f;
}
.pomo-settings-enter-active {
  transition: opacity var(--t-base) var(--ease-out-expo), transform var(--t-base) var(--ease-morph);
}
.pomo-settings-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}
.pomo-settings-enter-from,
.pomo-settings-leave-to {
  opacity: 0;
  transform: translateY(-8px) scale(0.96);
}

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

/* ---------------------------------------------------- 番茄序列（Draggable） */
.pomo-settings {
  flex-wrap: wrap;
}
.seq {
  flex-basis: 100%;
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  justify-content: center;
  margin-top: 3px;
}
/* 拖出不合法顺序：整排轻晃提示"弹回" */
.seq.snapping {
  animation: seq-shake 0.32s ease;
}
@keyframes seq-shake {
  25% {
    transform: translateX(-4px);
  }
  75% {
    transform: translateX(4px);
  }
}
.seq-card {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 0.28rem 0.6rem;
  border: 1px solid var(--glass-border);
  border-radius: 9px;
  background: var(--glass-bg-deep);
  color: var(--ink-soft);
  font-size: 0.72rem;
  cursor: grab;
  user-select: none;
  transition:
    border-color var(--t-fast) ease,
    background var(--t-fast) ease,
    opacity var(--t-fast) ease,
    transform var(--t-fast) var(--ease-out-back);
}
.seq-card:hover {
  border-color: var(--glass-border-strong);
  background: var(--glass-bg-strong);
}
.seq-card:active {
  cursor: grabbing;
}
.seq-card.dragging {
  opacity: 0.45;
  transform: scale(0.95);
}
.seq-handle {
  color: rgb(255 255 255 / 0.24);
  font-size: 0.68rem;
  letter-spacing: -2px;
}
.seq-card.focus .seq-name {
  color: var(--accent);
}
.seq-card.short .seq-name {
  color: #5aa7ff;
}
.seq-card.long {
  border-color: color-mix(in srgb, #b58cff 42%, transparent);
}
.seq-card.long .seq-name {
  color: #b58cff;
}
.seq-min {
  font-size: 0.64rem;
  color: var(--ink-faint);
  font-variant-numeric: tabular-nums;
}
.seq-hint {
  flex-basis: 100%;
  margin: 1px 0 0;
  text-align: center;
  font-size: 0.64rem;
  color: var(--ink-faint);
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
.theme-ctl {
  position: relative;
  display: inline-flex;
  align-items: center;
}
/* 第 21 轮：主题菜单按钮 —— 当前主题的色点 + 名称 + 下翻箭头，
   形态借 presets 按钮的胶囊壳，是底部菜单的一等公民而非角落色点 */
.theme-btn {
  display: inline-flex;
  align-items: center;
  gap: 0.45rem;
  padding: 0.3rem 0.65rem 0.3rem 0.45rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  background: var(--glass-bg);
  color: var(--ink-soft);
  font-size: 0.72rem;
  cursor: pointer;
  transition:
    border-color var(--t-base) var(--ease-out-expo),
    background var(--t-base) var(--ease-out-expo),
    color var(--t-base) ease,
    box-shadow var(--t-base) ease;
}
.theme-btn:hover,
.theme-btn.open {
  border-color: var(--glass-border-strong);
  background: var(--glass-bg-strong);
  color: var(--ink);
  box-shadow: var(--glass-shadow);
}
.theme-dot {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  border: 1px solid rgb(255 255 255 / 0.25);
  box-shadow: 0 0 8px color-mix(in srgb, var(--accent) 24%, transparent);
}
.theme-btn .chev {
  width: 9px;
  height: 6px;
  color: var(--ink-faint);
  transition: transform var(--t-base) var(--ease-out-back);
}
.theme-btn.open .chev {
  transform: rotate(180deg);
}
/* 弹出菜单：向上展开的玻璃面板，铺在底部菜单上方（高于内容层） */
.theme-menu {
  position: absolute;
  bottom: calc(100% + 10px);
  right: 0;
  z-index: var(--z-chrome);
  display: flex;
  flex-direction: column;
  min-width: 132px;
  padding: 4px;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius);
  background: var(--glass-bg-strong);
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-edge-strong), var(--glass-shadow-lg);
  transform-origin: 85% 100%;
}
.theme-item {
  display: flex;
  align-items: center;
  gap: 0.55rem;
  padding: 0.42rem 0.6rem;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--ink-soft);
  font-size: 0.74rem;
  text-align: left;
  cursor: pointer;
  transition: background var(--t-fast) ease, color var(--t-fast) ease;
}
.theme-item:hover {
  background: color-mix(in srgb, var(--ink) 8%, transparent);
  color: var(--ink);
}
/* ★ 选中项不能直接吃 --accent：空闲态 accent 是深灰 #4a5160，
   深色主题下文字会隐形 —— 文字用墨色、选中态用浅色底表达。
   ★ 特异性警示：元素同时命中 .presets button.active（0,3,1），
   这里必须挂 .presets 前缀抬到 (0,4,0) 才能盖过它。 */
.presets .theme-item.active {
  color: var(--ink);
  background: color-mix(in srgb, var(--ink) 10%, transparent);
}
.theme-item .sw {
  width: 15px;
  height: 15px;
  flex: none;
  border-radius: 50%;
  border: 1px solid var(--glass-border-strong);
}
.theme-item .check {
  margin-left: auto;
  font-size: 0.68rem;
}
/* 弹出过渡：从按钮锚点浮起 + 回弹收尾（--ease-out-back 的微过冲） */
.theme-pop-enter-active {
  transition: opacity 0.26s var(--ease-out-expo), transform 0.3s var(--ease-out-back);
}
.theme-pop-leave-active {
  transition: opacity 0.18s ease, transform 0.18s ease;
}
.theme-pop-enter-from,
.theme-pop-leave-to {
  opacity: 0;
  transform: translateY(8px) scale(0.94);
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
  .meditating .dial::after,
  .seq.snapping {
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
  .pomo-enter-active,
  .pomo-leave-active,
  .pomo-settings-enter-active,
  .pomo-settings-leave-active,
  .pill,
  .tips-indicator,
  .labels > span,
  .controls button,
  .presets button,
  .tag-chip,
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
</style>
