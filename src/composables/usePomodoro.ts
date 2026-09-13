// ---------------------------------------------------------------------------
// 番茄钟：状态展示、设置面板草稿、循环序列卡片拖拽。
//
// P1-⑤ 第二步起改为**模块级单例**：番茄状态被 App（侧栏/开关/accent 覆盖）
// 与 PomodoroPanel（阶段条/设置面板/序列卡）两处消费，必须共享同一份
// ref —— 菜单里改的参数要立刻反映到侧栏圆点。模块级状态 = 全局唯一。
//
// 循环推进在 Rust 侧完成（到点自动切阶段 + 系统通知），前端只负责展示
// 阶段与已完成的番茄数，以及开关与参数编辑。
// ---------------------------------------------------------------------------

import { computed, ref, watch } from "vue";
import { timerApi, type PomodoroConfig, type PomodoroPhase, type PomodoroStatus } from "../api";
import { toast } from "./useToast";

export const PHASE_LABEL: Record<PomodoroPhase, string> = {
  focus: "专注中",
  short_break: "短休息",
  long_break: "长休息",
};

/** 阶段强调色：专注用状态色，休息换蓝紫，一眼区分「该干劲」还是「该放松」。 */
const PHASE_ACCENT: Record<PomodoroPhase, string> = {
  focus: "", // 走默认 accent
  short_break: "#5aa7ff",
  long_break: "#b58cff",
};

// ---- 模块级单例状态 -------------------------------------------------------

const pomodoro = ref<PomodoroStatus | null>(null);

/** 番茄循环参数（Rust 侧持久化），设置面板读写它。 */
const pomoConfig = ref<PomodoroConfig | null>(null);
const pomoSettingsOpen = ref(false);
const pomoFeedback = ref<"" | "saved" | "error">("");

/** 设置面板的草稿：分钟为单位的四个输入框，点「应用」才真正下发。 */
const pomoDraft = ref({ focusMin: 25, shortMin: 5, longMin: 15, rounds: 4 });

function syncDraft(config: PomodoroConfig): void {
  pomoDraft.value = {
    focusMin: Math.round(config.focusMs / 60_000),
    shortMin: Math.round(config.shortBreakMs / 60_000),
    longMin: Math.round(config.longBreakMs / 60_000),
    rounds: config.focusBeforeLong,
  };
}

async function applyPomoConfig(): Promise<void> {
  const d = pomoDraft.value;
  // 数字输入框可能被清空成 NaN / 越界 —— 夹取到合法区间，UI 层先兜一道，
  // Rust 侧 validate 仍是最终防线。
  const clampMin = (v: number) => Math.min(120, Math.max(1, Math.round(v) || 1));
  const config: PomodoroConfig = {
    focusMs: clampMin(d.focusMin) * 60_000,
    shortBreakMs: clampMin(d.shortMin) * 60_000,
    longBreakMs: clampMin(d.longMin) * 60_000,
    focusBeforeLong: Math.min(8, Math.max(2, Math.round(d.rounds) || 2)),
  };
  try {
    pomodoro.value = await timerApi.pomodoroConfigSet(config);
    pomoConfig.value = config;
    syncDraft(config); // 把夹取后的值回填输入框
    pomoFeedback.value = "saved";
  } catch (err) {
    console.error("保存番茄配置失败", err);
    pomoFeedback.value = "error";
    toast.error("番茄参数保存失败", String(err));
  }
  setTimeout(() => (pomoFeedback.value = ""), 2200);
}

// -------------------------------------------------------------------------
// 番茄循环序列（Draggable）：把当前配置画成一张卡片序列
// [番茄钟, 短休息, ×(N-1), 长休息]。
//
// 引擎的节奏是固定的「专注 ↔ 短休交替、N 轮后长休」，所以可拖的只有
// 「长休息」这一张卡：把它拖到第 k 个番茄钟后面 = 长休前有 k 轮专注，
// 也就是改 focusBeforeLong。拖其它卡没有对应语义 —— 整排轻晃一下弹回原位。
// -------------------------------------------------------------------------

type SeqKind = "focus" | "short" | "long";
interface SeqCard {
  id: string;
  kind: SeqKind;
  minutes: number;
}
const SEQ_LABEL: Record<SeqKind, string> = {
  focus: "番茄钟",
  short: "短休息",
  long: "长休息",
};

const seqCards = computed<SeqCard[]>(() => {
  const c = pomoConfig.value;
  if (!c) return [];
  const focus = Math.round(c.focusMs / 60_000);
  const short = Math.round(c.shortBreakMs / 60_000);
  const long = Math.round(c.longBreakMs / 60_000);
  const n = c.focusBeforeLong;
  const cards: SeqCard[] = [];
  for (let i = 0; i < n; i += 1) {
    cards.push({ id: `f${i}`, kind: "focus", minutes: focus });
    if (i < n - 1) cards.push({ id: `s${i}`, kind: "short", minutes: short });
  }
  cards.push({ id: "long", kind: "long", minutes: long });
  return cards;
});

/** 拖拽用的本地副本：拖着的时候配置还没变，列表先跟着手走。 */
const seqLocal = ref<SeqCard[]>([]);
watch(
  seqCards,
  (cards) => {
    seqLocal.value = [...cards];
  },
  { immediate: true },
);

const dragFrom = ref(-1);
const seqSnapping = ref(false);
let snapTimer: number | undefined;

/** 不合法的拖法：整排晃一下、列表弹回规范序。 */
function snapBackSeq(): void {
  seqLocal.value = [...seqCards.value];
  seqSnapping.value = true;
  clearTimeout(snapTimer);
  snapTimer = window.setTimeout(() => (seqSnapping.value = false), 340);
}

function onDropSeq(to: number): void {
  const from = dragFrom.value;
  dragFrom.value = -1;
  if (from < 0 || from === to) return;
  const list = [...seqLocal.value];
  const [moved] = list.splice(from, 1);
  list.splice(to, 0, moved);
  // 只有「长休息换位置」能映射回配置；其余排法一律弹回
  if (moved.kind !== "long") return snapBackSeq();
  const rounds = list
    .slice(0, list.findIndex((c) => c.kind === "long"))
    .filter((c) => c.kind === "focus").length;
  if (rounds < 2 || rounds > 8) return snapBackSeq();
  seqLocal.value = list;
  void persistRounds(rounds);
}

/** 落库走既有配置通道（Rust 侧校验 + 持久化），成功后全 UI 自动跟进。 */
async function persistRounds(rounds: number): Promise<void> {
  const c = pomoConfig.value;
  if (!c) return;
  const next = { ...c, focusBeforeLong: rounds };
  try {
    pomodoro.value = await timerApi.pomodoroConfigSet(next);
    pomoConfig.value = next;
    syncDraft(next);
    toast.info("长休位置已更新", `每 ${rounds} 轮专注后进入长休`);
  } catch (err) {
    toast.error("保存失败", String(err));
    seqLocal.value = [...seqCards.value];
  }
}

async function togglePomodoro(): Promise<void> {
  const enable = !(pomodoro.value?.enabled ?? false);
  try {
    pomodoro.value = await timerApi.pomodoroSet(enable);
    toast.info(
      enable ? "已开启番茄钟" : "已关闭番茄钟",
      enable
        ? `${Math.round((pomoConfig.value?.focusMs ?? 0) / 60_000)} 分钟专注 + 循环休息`
        : "回到自由计时",
    );
  } catch (err) {
    console.error("切换番茄模式失败", err);
    toast.error("切换番茄钟失败", String(err));
  }
}

/** 进度条上的番茄圆点：长休期间全亮，其余显示本轮已完成的数量。 */
const totalDots = computed(() => pomoConfig.value?.focusBeforeLong ?? 4);
const litDots = computed(() => {
  if (!pomodoro.value?.enabled) return 0;
  if (pomodoro.value.phase === "long_break") return totalDots.value;
  return pomodoro.value.completedFocus % totalDots.value;
});

const phaseAccent = computed(() => {
  const phase = pomodoro.value?.phase;
  return (phase && PHASE_ACCENT[phase]) || null;
});

// snapTimer 只做「晃动复位」的防御性清理：进程退出时 timeout 随之消亡，
// 无需组件级 onUnmounted（单例状态没有对应的组件生命周期可挂）。

// ---- 消费入口（多次调用返回同一份状态）------------------------------------

export function usePomodoro() {
  return {
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
  };
}
