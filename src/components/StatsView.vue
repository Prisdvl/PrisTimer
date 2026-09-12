<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import {
  timerApi,
  type DailyStat,
  type HourTotal,
  type StatsSummary,
  type TagDayTotal,
  type TagGoals,
  type TagTotal,
} from "../api";
import { addDays, formatDuration, toDayKey } from "../date";
import { toast } from "../composables/useToast";
import { TAG_PRESETS } from "../tags";
import { useCountUp } from "../composables/useCountUp";
import HourBars, { type HourRow } from "./HourBars.vue";
import TrendChart, { type TrendRow } from "./TrendChart.vue";
import YearHeatmap from "./YearHeatmap.vue";

type Range = 7 | 30 | 90;
type Mode = "bar" | "line";

const RANGES: Array<{ label: string; days: Range }> = [
  { label: "7 天", days: 7 },
  { label: "30 天", days: 30 },
  { label: "90 天", days: 90 },
];

const summary = ref<StatsSummary | null>(null);
const rangeStats = ref<DailyStat[]>([]);
/** 近一年逐日数据：年度热力图与「本月」卡片共用一份查询。 */
const yearStats = ref<DailyStat[]>([]);
const tagTotals = ref<TagTotal[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);

const range = ref<Range>(7);
const mode = ref<Mode>("bar");

/** 科目统计的时间范围。0 表示全部。 */
type TagRange = 7 | 30 | 0;
const TAG_RANGES: Array<{ label: string; days: TagRange }> = [
  { label: "7 天", days: 7 },
  { label: "30 天", days: 30 },
  { label: "全部", days: 0 },
];
const tagRange = ref<TagRange>(7);

/** 热力图窗口：最近 365 天（含今天）。 */
const YEAR_DAYS = 365;

const today = toDayKey(new Date());

/** 当前选中的范围数据转成查表结构，卡片与趋势图共用。 */
const totalsByDay = computed(
  () => new Map(rangeStats.value.map((stat) => [stat.day, stat.totalMs])),
);

const todayTotalMs = computed(() => totalsByDay.value.get(today) ?? 0);

/** 近 7 天合计（范围 ≥7 天时总覆盖到）。 */
const weekTotalMs = computed(() => {
  const base = new Date();
  let sum = 0;
  for (let offset = 0; offset < 7; offset += 1) {
    sum += totalsByDay.value.get(toDayKey(addDays(base, -offset))) ?? 0;
  }
  return sum;
});

/** 「本月」= 当前自然月的合计，从近一年那份数据里按前缀取，省一次查询。 */
const monthTotalMs = computed(() => {
  const prefix = today.slice(0, 7);
  return yearStats.value
    .filter((stat) => stat.day.startsWith(prefix))
    .reduce((sum, stat) => sum + stat.totalMs, 0);
});

// 卡片数字用缓动补间：数据到位后从旧值滚到新值，而不是瞬间跳变。
const totalSourceMs = computed(() => summary.value?.totalMs ?? 0);
const todayText = useCountUp(todayTotalMs, formatDuration);
const weekText = useCountUp(weekTotalMs, formatDuration);
const monthText = useCountUp(monthTotalMs, formatDuration);
const totalText = useCountUp(totalSourceMs, formatDuration);

/** 趋势图行数据：稀疏标注横轴，90 天时只标 ~7 个刻度。 */
const trendRows = computed<TrendRow[]>(() => {
  const n = range.value;
  const every = Math.max(1, Math.ceil(n / 7));
  const base = new Date();
  const rows: TrendRow[] = [];
  for (let offset = n - 1; offset >= 0; offset -= 1) {
    const date = addDays(base, -offset);
    const key = toDayKey(date);
    rows.push({
      day: key,
      totalMs: totalsByDay.value.get(key) ?? 0,
      label: `${date.getMonth() + 1}/${date.getDate()}`,
      showLabel: offset % every === every - 1 || offset === 0,
    });
  }
  return rows;
});

/** 科目分布的条形宽度百分比（以最长科目为基准）。 */
const tagBars = computed(() => {
  const max = Math.max(1, ...tagTotals.value.map((t) => t.totalMs));
  return tagTotals.value.map((t) => ({
    ...t,
    name: t.tag === "" ? "未标注" : t.tag,
    percent: Math.max(6, Math.round((t.totalMs / max) * 100)),
  }));
});

// ------------------------------------------------------------ 重命名 / 合并
// 「合并」就是「重命名到一个已存在的名字」，Rust 侧同一句 UPDATE 幂等处理。

const renameTarget = ref<{ tag: string; name: string; count: number } | null>(null);
const renameDraft = ref("");
const renameSaving = ref(false);
const renameMsg = ref<string | null>(null);
const renameOk = ref(false);

// ---------------------------------------------------------------- 近 7 天回顾
// 一次拉 90 天逐日数据：streak 往前回溯、周环比、最专注的一天全从它算；
// 目标达成天数用「日 × 科目」聚合，只查近 7 天。

const longStats = ref<DailyStat[]>([]);
const weekTagDays = ref<TagDayTotal[]>([]);

async function loadReview(): Promise<void> {
  const base = new Date();
  longStats.value = await timerApi.dailyStats(toDayKey(addDays(base, -89)), toDayKey(base));
  weekTagDays.value = await timerApi.statsTagDaily(toDayKey(addDays(base, -6)), toDayKey(base));
}

const longMsByDay = computed(
  () => new Map(longStats.value.map((s) => [s.day, s.totalMs])),
);

function sumDays(offsetEnd: number, count: number): number {
  let sum = 0;
  const base = new Date();
  for (let i = 0; i < count; i += 1) {
    sum += longMsByDay.value.get(toDayKey(addDays(base, -(offsetEnd + i)))) ?? 0;
  }
  return sum;
}

const weekMs = computed(() => sumDays(0, 7));
const prevWeekMs = computed(() => sumDays(7, 7));
const weekDeltaPct = computed(() =>
  prevWeekMs.value > 0 ? Math.round(((weekMs.value - prevWeekMs.value) / prevWeekMs.value) * 100) : null,
);
const weekDeltaText = computed(() =>
  weekDeltaPct.value === null ? "前 7 天没有记录" : `${weekDeltaPct.value >= 0 ? "+" : ""}${weekDeltaPct.value}%`,
);

/** 连续专注天数：从今天（没学则从昨天）往前数不间断的有记录日。 */
const streak = computed(() => {
  let n = 0;
  for (let i = 0; i < 90; i += 1) {
    const key = toDayKey(addDays(new Date(), -i));
    if ((longMsByDay.value.get(key) ?? 0) > 0) {
      n += 1;
    } else if (i === 0) {
      continue; // 今天还没开始学不算断签
    } else {
      break;
    }
  }
  return n;
});

const bestDay = computed(() => {
  let best: DailyStat | null = null;
  for (const stat of longStats.value) {
    if (!best || stat.totalMs > best.totalMs) {
      best = stat;
    }
  }
  if (!best || best.totalMs === 0) {
    return null;
  }
  const [, m, d] = best.day.split("-").map(Number);
  return { text: formatDuration(best.totalMs), label: `${m}/${d}` };
});

const activeDays = computed(() => {
  let n = 0;
  const base = new Date();
  for (let i = 0; i < 7; i += 1) {
    if ((longMsByDay.value.get(toDayKey(addDays(base, -i))) ?? 0) > 0) {
      n += 1;
    }
  }
  return n;
});

const avgMs = computed(() => Math.round(weekMs.value / 7));

// 回顾块的数字同样走 Counter 补间（参考集「数字计数器」）：
// 时长用 formatDuration 滚动，天数按整数滚（epsilon 收到 0.5，变化即动画）。
const weekMsText = useCountUp(weekMs, formatDuration, 1000);
const streakText = useCountUp(streak, (n) => String(Math.round(n)), 900, 0.5);
const avgMsText = useCountUp(avgMs, formatDuration, 900);

/** 每个目标科目近 7 天达成目标的天数。 */
const goalWeekRows = computed(() =>
  Object.entries(goals.value)
    .map(([tag, minutes]) => {
      const days = new Set(
        weekTagDays.value
          .filter((r) => r.tag === tag && r.totalMs >= minutes * 60_000)
          .map((r) => r.day),
      ).size;
      return { tag, days };
    })
    .sort((a, b) => b.days - a.days),
);

function openRename(row: { tag: string; name: string; sessionCount: number }): void {
  renameTarget.value = {
    tag: row.tag,
    name: row.name,
    count: row.sessionCount,
  };
  // 未标注行没有可回填的名字，输入框留空引导输入目标科目
  renameDraft.value = row.tag === "" ? "" : row.tag;
  renameMsg.value = null;
}

function closeRename(): void {
  renameTarget.value = null;
  renameDraft.value = "";
  renameMsg.value = null;
}

async function confirmRename(): Promise<void> {
  const target = renameTarget.value;
  if (!target) {
    return;
  }
  const to = renameDraft.value.trim().slice(0, 20) || null;
  if (to === null && target.tag === "") {
    renameMsg.value = "「未标注」没有可清除的标签，请输入目标科目名";
    renameOk.value = false;
    return;
  }
  if (to === target.tag) {
    renameMsg.value = "新名字与现在相同，没有需要改的";
    renameOk.value = false;
    return;
  }
  renameSaving.value = true;
  try {
    const changed = await timerApi.tagRename(target.tag, to);
    await Promise.all([loadTags(), loadTodayTags(), loadGoals()]);
    renameMsg.value = `已改写 ${changed} 条会话 ✓`;
    renameOk.value = true;
    // 给用户看清反馈再收起面板
    setTimeout(() => {
      if (renameOk.value) {
        closeRename();
      }
    }, 1600);
  } catch (err) {
    renameMsg.value = String(err);
    renameOk.value = false;
  } finally {
    renameSaving.value = false;
  }
}

async function loadRange(): Promise<void> {
  const base = new Date();
  rangeStats.value = await timerApi.dailyStats(
    toDayKey(addDays(base, -(range.value - 1))),
    toDayKey(base),
  );
}

async function loadYear(): Promise<void> {
  const base = new Date();
  yearStats.value = await timerApi.dailyStats(
    toDayKey(addDays(base, -(YEAR_DAYS - 1))),
    toDayKey(base),
  );
}

async function refresh(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    // 六个请求互不依赖，并发发出。
    await Promise.all([
      loadSummary(),
      loadRange(),
      loadYear(),
      loadTags(),
      loadGoals(),
      loadTodayTags(),
      loadReview(),
      loadHourly(),
    ]);
  } catch (err) {
    error.value = String(err);
  } finally {
    loading.value = false;
  }
}

async function loadSummary(): Promise<void> {
  summary.value = await timerApi.summary();
}

async function loadTags(): Promise<void> {
  const base = new Date();
  const from =
    tagRange.value === 0
      ? "1970-01-01" // 「全部」：数据库里的实际数据不可能早于这个本地日
      : toDayKey(addDays(base, -(tagRange.value - 1)));
  tagTotals.value = await timerApi.statsTags(from, toDayKey(base));
}

// ---------------------------------------------------------------- 今日目标
// 目标是「科目 → 每日分钟数」，存在 settings 表的 tag_goals 键；
// 当日实际专注时长复用 statsTags(today, today)，无需新查询。

const goals = ref<TagGoals>({});
const todayTagTotals = ref<TagTotal[]>([]);

async function loadGoals(): Promise<void> {
  goals.value = await timerApi.tagGoalsGet();
}

async function loadTodayTags(): Promise<void> {
  todayTagTotals.value = await timerApi.statsTags(today, today);
}

/** 当日各科目实际专注毫秒数（查表结构，无记录的科目取 0）。 */
const todayMsByTag = computed(
  () => new Map(todayTagTotals.value.map((t) => [t.tag, t.totalMs])),
);

/** 今日目标进度行：百分比封顶 100，达成时点亮。 */
const goalRows = computed(() =>
  Object.entries(goals.value).map(([tag, minutes]) => {
    const doneMs = todayMsByTag.value.get(tag) ?? 0;
    const targetMs = minutes * 60_000;
    const percent = Math.min(100, Math.round((doneMs / targetMs) * 100));
    return {
      tag,
      minutes,
      doneText: formatDuration(doneMs),
      percent,
      achieved: doneMs >= targetMs,
    };
  }),
);

const goalsOpen = ref(false);
const goalsDraft = reactive<Record<string, string>>({});
const goalsSaving = ref(false);
const goalsMsg = ref<string | null>(null);
const goalsOk = ref(false);

function syncGoalsDraft(): void {
  for (const preset of TAG_PRESETS) {
    goalsDraft[preset] = goals.value[preset] ? String(goals.value[preset]) : "";
  }
}

function toggleGoals(): void {
  goalsOpen.value = !goalsOpen.value;
  if (goalsOpen.value) {
    syncGoalsDraft();
    goalsMsg.value = null;
  }
}

async function saveGoals(): Promise<void> {
  const draft: TagGoals = {};
  for (const preset of TAG_PRESETS) {
    // 注意：v-model 在 type="number" 输入框上会把值自动转成 number，
    // 这里必须先 String 化再 trim，否则 (30).trim 直接 TypeError。
    const raw = String(goalsDraft[preset] ?? "").trim();
    if (raw === "" || raw === "0") {
      continue; // 留空 = 不设目标
    }
    const minutes = Number(raw);
    if (!Number.isInteger(minutes) || minutes < 1 || minutes > 1440) {
      goalsMsg.value = `${preset} 的分钟数应为 1–1440 的整数`;
      goalsOk.value = false;
      return;
    }
    draft[preset] = minutes;
  }
  goalsSaving.value = true;
  try {
    goals.value = await timerApi.tagGoalsSet(draft);
    syncGoalsDraft();
    goalsMsg.value = "已保存 ✓";
    goalsOk.value = true;
  } catch (err) {
    goalsMsg.value = String(err);
    goalsOk.value = false;
  } finally {
    goalsSaving.value = false;
  }
}

async function setTagRange(days: TagRange): Promise<void> {
  if (tagRange.value === days) {
    return;
  }
  tagRange.value = days;
  error.value = null;
  try {
    await loadTags();
  } catch (err) {
    error.value = String(err);
  }
}

// ---------------------------------------------------------- 专注时段分布
// 「我通常在哪个钟点坐下开始学」—— 按会话开始时刻的本地小时（0–23）聚合。
// 窗口固定近 30 天：时段分析在长窗口下才有统计意义；原先的「7 天/30 天/全部」
// 切换器与趋势图的范围切换功能重合且无响应，按需求删除。
const HOUR_WINDOW_DAYS = 30;
const hourStats = ref<HourTotal[]>([]);

async function loadHourly(): Promise<void> {
  const base = new Date();
  hourStats.value = await timerApi.statsHourly(
    toDayKey(addDays(base, -(HOUR_WINDOW_DAYS - 1))),
    toDayKey(base),
  );
}

/** 补全 0–23 的空档：没记录的小时给 0，图上留一根底座。 */
const hourRows = computed<HourRow[]>(() => {
  const byHour = new Map(hourStats.value.map((r) => [r.hour, r]));
  return Array.from({ length: 24 }, (_, hour) => ({
    hour,
    totalMs: byHour.get(hour)?.totalMs ?? 0,
    sessionCount: byHour.get(hour)?.sessionCount ?? 0,
  }));
});

const hourSumMs = computed(() =>
  hourStats.value.reduce((sum, r) => sum + r.totalMs, 0),
);

/** 峰值洞察：「最专注时段 14:00–15:00 · 3 小时 20 分（占 28%）」。并列取最早。 */
const hourPeak = computed(() => {
  let best: HourTotal | null = null;
  for (const r of hourStats.value) {
    if (r.totalMs > 0 && (!best || r.totalMs > best.totalMs)) {
      best = r;
    }
  }
  if (!best || hourSumMs.value === 0) {
    return null;
  }
  return {
    from: best.hour,
    text: formatDuration(best.totalMs),
    pct: Math.round((best.totalMs / hourSumMs.value) * 100),
  };
});

const pad2 = (n: number): string => String(n % 24).padStart(2, "0");

async function setRange(days: Range): Promise<void> {
  if (range.value === days) {
    return;
  }
  range.value = days;
  error.value = null;
  try {
    await loadRange();
  } catch (err) {
    error.value = String(err);
  }
}

onMounted(refresh);

// ---------------------------------------------------------------- 导出 CSV
// 落盘结果统一走 toast：它是全局提示，不占版面、不挤压相邻按钮，
// 长路径可以在自己的面板里折行显示。
const exporting = ref(false);

/** 生成 `pristimer-sessions-20260911-0930.csv` 风格的时间戳文件名。 */
function exportFilename(): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  const d = new Date();
  return (
    `pristimer-sessions-${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}` +
    `-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}.csv`
  );
}

async function exportCsv(): Promise<void> {
  exporting.value = true;
  try {
    const path = await timerApi.exportCsv(exportFilename());
    toast.success("会话明细已导出", path);
  } catch (err) {
    toast.error("导出失败", String(err));
  } finally {
    exporting.value = false;
  }
}

// -------------------------------------------------------------- 导出周报
// 固定覆盖「近 7 天」（含今天），与上面回顾块的窗口一致 —— 用户点导出，
// 拿到的就是屏幕上正在看的那一周。

const reportFrom = toDayKey(addDays(new Date(), -6));
const reportTo = today;
/** 「9/5 – 9/11」风格的短范围文本，挂在按钮 title 上。 */
const reportRangeText = `${reportFrom.slice(5).replace("-", "/")} – ${reportTo
  .slice(5)
  .replace("-", "/")}`;

const reportExporting = ref(false);

/** `pristimer-report-20260911.csv` —— 按「截止日」命名，同一天重复导出会覆盖。 */
function reportFilename(): string {
  return `pristimer-report-${reportTo.replace(/-/g, "")}.csv`;
}

async function exportReport(): Promise<void> {
  reportExporting.value = true;
  try {
    const path = await timerApi.exportReportCsv(
      reportFilename(),
      reportFrom,
      reportTo,
    );
    toast.success("周报已导出", path);
  } catch (err) {
    toast.error("周报导出失败", String(err));
  } finally {
    reportExporting.value = false;
  }
}
</script>

<template>
  <section class="stats">
    <p v-if="error" class="error">读取统计失败：{{ error }}</p>

    <div class="cards">
      <article
        v-for="card in 4"
        :key="card"
        class="card sheen"
        :style="{ animationDelay: `${(card - 1) * 60}ms` }"
      >
        <template v-if="card === 1">
          <p class="label">今日专注</p>
          <p class="value">{{ todayText }}</p>
        </template>
        <template v-else-if="card === 2">
          <p class="label">近 7 天</p>
          <p class="value">{{ weekText }}</p>
        </template>
        <template v-else-if="card === 3">
          <p class="label">本月</p>
          <p class="value">{{ monthText }}</p>
        </template>
        <template v-else>
          <p class="label">累计</p>
          <p class="value">
            {{ summary ? totalText : "—" }}
            <small v-if="summary">{{ summary.sessionCount }} 次</small>
          </p>
        </template>
      </article>
    </div>

    <section class="block">
      <header class="block-head">
        <h3>趋势</h3>
        <div class="switches">
          <div class="seg" :style="{ '--n': RANGES.length, '--i': RANGES.findIndex(r => r.days === range) }">
            <span class="seg-ind" />
            <button
              v-for="item in RANGES"
              :key="item.days"
              :class="{ active: range === item.days }"
              @click="setRange(item.days)"
            >
              {{ item.label }}
            </button>
          </div>
          <div class="seg" :style="{ '--n': 2, '--i': mode === 'bar' ? 0 : 1 }">
            <span class="seg-ind" />
            <button :class="{ active: mode === 'bar' }" @click="mode = 'bar'">柱状</button>
            <button :class="{ active: mode === 'line' }" @click="mode = 'line'">折线</button>
          </div>
        </div>
      </header>

      <!-- key 带上 range：换范围时整图重放进场动画（柱体交错生长 / 折线重新描线） -->
      <Transition name="range" mode="out-in">
        <TrendChart :key="range" :rows="trendRows" :mode="mode" :today="today" />
      </Transition>
    </section>

    <section class="block">
      <h3>专注日历</h3>
      <YearHeatmap :stats="yearStats" :today="today" :days="YEAR_DAYS" />
    </section>

    <section class="block">
      <header class="block-head">
        <h3>今日目标</h3>
        <button
          class="goals-toggle"
          :class="{ active: goalsOpen }"
          title="设置每日目标"
          @click="toggleGoals"
        >
          <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
            <path d="M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58c.18-.14.23-.41.12-.61l-1.92-3.32c-.12-.22-.37-.29-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54c-.04-.24-.24-.41-.48-.41h-3.84c-.24 0-.43.17-.47.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96c-.22-.08-.47 0-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.03 1.58c-.18.14-.23.41-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.56 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6c-1.98 0-3.6-1.62-3.6-3.6s1.62-3.6 3.6-3.6 3.6 1.62 3.6 3.6-1.62 3.6-3.6 3.6z" />
          </svg>
        </button>
      </header>

      <!-- 目标编辑面板：留空表示不设该科目的目标 -->
      <Transition name="range">
        <div v-if="goalsOpen" class="goals-panel">
          <div v-for="preset in TAG_PRESETS" :key="preset" class="goal-edit-row">
            <span class="goal-edit-name">{{ preset }}</span>
            <input
              v-model="goalsDraft[preset]"
              class="goal-input"
              type="number"
              min="1"
              max="1440"
              placeholder="未设置"
              @keyup.enter="saveGoals"
            />
            <span class="goal-unit">分钟 / 天</span>
          </div>
          <div class="goal-edit-foot">
            <button class="refresh" :disabled="goalsSaving" @click="saveGoals">
              {{ goalsSaving ? "保存中…" : "保存目标" }}
            </button>
            <Transition name="range">
              <span v-if="goalsMsg" class="goal-msg" :class="{ ok: goalsOk }">{{ goalsMsg }}</span>
            </Transition>
          </div>
        </div>
      </Transition>

      <ul v-if="goalRows.length" class="tag-list">
        <li v-for="row in goalRows" :key="row.tag" :class="{ done: row.achieved }">
          <span class="tag-name">{{ row.tag }}</span>
          <span class="tag-bar-track">
            <span
              class="tag-bar goal-bar"
              :class="{ done: row.achieved }"
              :style="{ width: `${row.percent}%` }"
            />
          </span>
          <span class="tag-value">
            {{ row.doneText }}
            <small>/ {{ row.minutes }} 分钟 · {{ row.percent }}%</small>
            <small v-if="row.achieved" class="goal-check">✓ 已达成</small>
          </span>
        </li>
      </ul>
      <p v-else-if="!goalsOpen" class="goal-empty">
        还没有目标 —— 点右上角齿轮给科目设个每日分钟数
      </p>
    </section>

    <section class="block">
      <h3>近 7 天回顾</h3>
      <div class="review-grid">
        <div class="review-cell">
          <p class="rv-label">近 7 天专注</p>
          <p class="rv-value">{{ weekMsText }}</p>
          <p class="rv-sub" :class="weekDeltaPct === null ? '' : weekDeltaPct >= 0 ? 'up' : 'down'">
            {{ weekDeltaText }}
            <small v-if="weekDeltaPct !== null">vs 前 7 天</small>
          </p>
        </div>
        <div class="review-cell">
          <p class="rv-label">连续专注</p>
          <p class="rv-value">{{ streakText }}<small> 天</small></p>
          <p class="rv-sub">不间断打卡</p>
        </div>
        <div class="review-cell">
          <p class="rv-label">最专注的一天</p>
          <p class="rv-value">{{ bestDay ? bestDay.text : "—" }}</p>
          <p class="rv-sub">{{ bestDay ? `出现在 ${bestDay.label}` : "还没有记录" }}</p>
        </div>
        <div class="review-cell">
          <p class="rv-label">日均</p>
          <p class="rv-value">{{ avgMsText }}</p>
          <p class="rv-sub">按有记录的 {{ activeDays }} 天计</p>
        </div>
      </div>

      <ul v-if="goalWeekRows.length" class="review-goals">
        <li v-for="row in goalWeekRows" :key="row.tag">
          <span class="tag-name">{{ row.tag }}</span>
          <span class="goal-dots">
            <i v-for="n in 7" :key="n" :class="{ on: n <= row.days }" />
          </span>
          <span class="rv-goal-days">达成 <b>{{ row.days }}</b>/7 天</span>
        </li>
      </ul>
      <p v-else class="goal-empty">设好目标后，这里会显示每个科目本周达成目标的天数</p>
    </section>

    <section v-if="tagBars.length" class="block">
      <header class="block-head">
        <h3>科目统计</h3>
        <div class="seg" :style="{ '--n': TAG_RANGES.length, '--i': TAG_RANGES.findIndex(r => r.days === tagRange) }">
          <span class="seg-ind" />
          <button
            v-for="item in TAG_RANGES"
            :key="item.days"
            :class="{ active: tagRange === item.days }"
            @click="setTagRange(item.days)"
          >
            {{ item.label }}
          </button>
        </div>
      </header>
      <!-- ★ key 不带范围：换范围时行复用，条宽从旧值平滑过渡到新值（width 有
           0.7s transition）、顺序变化走 tagrow-move 补位 —— 之前 key 带范围会让
           整组拆掉重挂，加上数据是异步取回的，中间出现"列表塌空再撑开"的跳变，
           双重动画叠在一起显得很不自然。新出现的行仍走 tagrow-enter 入场。 -->
      <TransitionGroup tag="ul" name="tagrow" class="tag-list">
        <li
          v-for="(row, i) in tagBars"
          :key="row.name"
          :style="{ '--delay': `${i * 55}ms` }"
        >
          <span class="tag-name">{{ row.name }}</span>
          <span class="tag-bar-track">
            <span class="tag-bar" :style="{ width: `${row.percent}%` }" />
          </span>
          <span class="tag-value">
            {{ formatDuration(row.totalMs) }}
            <small>{{ row.sessionCount }} 次</small>
          </span>
          <button
            class="row-edit"
            title="重命名 / 合并到其他科目"
            @click="openRename(row)"
          >
            <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor">
              <path d="M3 17.25V21h3.75L17.81 9.94l-3.75-3.75L3 17.25zM20.71 7.04a1 1 0 0 0 0-1.41l-2.34-2.34a1 1 0 0 0-1.41 0l-1.83 1.83 3.75 3.75 1.83-1.83z" />
            </svg>
          </button>
        </li>
      </TransitionGroup>

      <!-- 重命名 / 合并编辑：输入已有科目名即合并，留空清除该标签 -->
      <Transition name="range">
        <div v-if="renameTarget" class="rename-panel">
          <p class="rename-line">
            将「{{ renameTarget.name }}」的 {{ renameTarget.count }} 条会话改为：
          </p>
          <div class="rename-row">
            <input
              v-model="renameDraft"
              class="goal-input rename-input"
              type="text"
              maxlength="20"
              :placeholder="renameTarget.tag === '' ? '新科目名' : '新名字（留空=清除标签）'"
              @keyup.enter="confirmRename"
            />
            <button class="refresh" :disabled="renameSaving" @click="confirmRename">
              {{ renameSaving ? "保存中…" : "保存" }}
            </button>
            <button class="refresh" @click="closeRename">取消</button>
          </div>
          <p class="rename-hint">输入已有科目名即合并；留空则清除标签（变为未标注）。</p>
          <Transition name="range">
            <p v-if="renameMsg" class="goal-msg" :class="{ ok: renameOk }">{{ renameMsg }}</p>
          </Transition>
        </div>
      </Transition>
    </section>

    <section class="block">
      <header class="block-head">
        <h3>专注时段</h3>
        <span class="block-sub">近 {{ HOUR_WINDOW_DAYS }} 天</span>
      </header>
      <p v-if="hourPeak" class="hour-peak">
        最专注时段
        <b>{{ pad2(hourPeak.from) }}:00–{{ pad2(hourPeak.from + 1) }}:00</b>
        · {{ hourPeak.text }}（占 {{ hourPeak.pct }}%）
      </p>
      <HourBars v-if="hourSumMs > 0" :rows="hourRows" />
      <p v-else class="goal-empty">该范围内还没有记录</p>
    </section>

    <div class="foot">
      <button class="refresh" :disabled="loading" @click="refresh">
        {{ loading ? "读取中…" : "刷新" }}
      </button>
      <button class="refresh" :disabled="exporting" @click="exportCsv">
        {{ exporting ? "导出中…" : "导出 CSV" }}
      </button>
      <button
        class="refresh"
        :disabled="reportExporting"
        :title="`导出近 7 天（${reportRangeText}）的周报`"
        @click="exportReport"
      >
        {{ reportExporting ? "生成中…" : "导出周报" }}
      </button>
      <!-- 导出结果改成右上角的 toast，不再往这一行里塞长路径 -->
    </div>
  </section>
</template>

<style scoped>
.stats {
  display: flex;
  flex-direction: column;
  gap: 1.5rem;
}
.error {
  color: #ff8a8a;
  font-size: 0.85rem;
  margin: 0;
}

/* ------------------------------------------------------------------ 卡片 */
.cards {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 0.75rem;
}
.card {
  background: var(--glass-bg, rgb(255 255 255 / 0.045));
  border: 1px solid var(--glass-border, rgb(255 255 255 / 0.08));
  border-radius: 12px;
  padding: 0.9rem 1rem;
  backdrop-filter: var(--glass-blur, none);
  animation: rise 0.5s var(--ease-out-expo, cubic-bezier(0.16, 1, 0.3, 1)) backwards;
  transition: border-color 0.25s ease, transform 0.35s var(--ease-out-expo, ease),
    background 0.25s ease;
}
.card:hover {
  border-color: rgb(255 255 255 / 0.16);
  background: var(--glass-bg-strong, rgb(255 255 255 / 0.08));
  transform: translateY(-3px);
}
@keyframes rise {
  from {
    opacity: 0;
    transform: translateY(10px);
  }
}
.card .label {
  margin: 0;
  font-size: 0.72rem;
  color: #8b93a3;
  letter-spacing: 0.1em;
}
.card .value {
  margin: 0.35rem 0 0;
  font-size: 1.15rem;
  font-variant-numeric: tabular-nums;
}
.card small {
  margin-left: 0.4rem;
  font-size: 0.7rem;
  color: #8b93a3;
}

/* -------------------------------------------------------------- 趋势区块 */
/* 块级入场：整页挂载时自上而下依次浮起（nth-child 提供错峰延迟），
   统计页从计时页切进来时不再是"啪"地整块出现 */
.stats > .block {
  animation: rise 0.55s var(--ease-out-expo) backwards;
}
.stats > .block:nth-child(1) {
  animation-delay: 0ms;
}
.stats > .block:nth-child(2) {
  animation-delay: 55ms;
}
.stats > .block:nth-child(3) {
  animation-delay: 110ms;
}
.stats > .block:nth-child(4) {
  animation-delay: 165ms;
}
.stats > .block:nth-child(5) {
  animation-delay: 220ms;
}
.stats > .block:nth-child(6) {
  animation-delay: 275ms;
}
.stats > .block:nth-child(7) {
  animation-delay: 330ms;
}
.stats > .block:nth-child(8) {
  animation-delay: 385ms;
}

.block h3 {
  margin: 0 0 0.6rem;
  font-size: 0.8rem;
  font-weight: 500;
  color: #8b93a3;
  letter-spacing: 0.1em;
}
.block-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 0.5rem;
  margin-bottom: 0.6rem;
}
.block-head h3 {
  margin: 0;
}
/* 区块标题右侧的静态说明（如「专注时段 · 近 30 天」） */
.block-sub {
  font-size: 0.72rem;
  color: var(--ink-faint);
}
.switches {
  display: flex;
  gap: 0.5rem;
}
.seg {
  position: relative;
  display: grid;
  grid-auto-flow: column;
  grid-auto-columns: 1fr;
  padding: 2px;
  background: var(--glass-bg, rgb(255 255 255 / 0.045));
  border: 1px solid var(--glass-border, rgb(255 255 255 / 0.08));
  border-radius: 8px;
  backdrop-filter: var(--glass-blur, none);
}
/* 滑动的玻璃药丸指示器：切范围/形态时它平移过去，active 底色不再硬切 */
.seg-ind {
  position: absolute;
  top: 2px;
  bottom: 2px;
  left: 2px;
  width: calc((100% - 4px) / var(--n, 2));
  border-radius: 6px;
  background: var(--glass-bg-strong, rgb(255 255 255 / 0.09));
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.07);
  transform: translateX(calc(var(--i, 0) * 100%));
  transition: transform 0.35s var(--ease-out-expo, ease);
  pointer-events: none;
}
.seg button {
  position: relative;
  padding: 0.22rem 0.75rem;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: #8b93a3;
  font-size: 0.75rem;
  cursor: pointer;
  white-space: nowrap;
  transition: color 0.25s ease;
}
.seg button:hover {
  color: #cfd4dd;
}
.seg button.active {
  color: #3ecf8e;
}

/* 范围切换：整图旧数据快速下沉淡出、新图重放进场动画 */
.range-enter-active {
  transition: opacity 0.25s var(--ease-out-expo, ease), transform 0.25s var(--ease-out-expo, ease);
}
.range-leave-active {
  transition: opacity 0.18s ease, transform 0.18s ease;
}
.range-enter-from {
  opacity: 0;
  transform: translateY(8px);
}
.range-leave-to {
  opacity: 0;
  transform: translateY(-6px);
}

/* ------------------------------------------------------------------ 今日目标 */
.goals-toggle {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  padding: 0;
  border: 1px solid var(--glass-border, rgb(255 255 255 / 0.08));
  border-radius: 8px;
  background: var(--glass-bg, rgb(255 255 255 / 0.045));
  color: #8b93a3;
  cursor: pointer;
  transition: color 0.2s ease, border-color 0.2s ease, transform 0.4s var(--ease-out-expo, ease);
}
.goals-toggle:hover {
  color: #cfd4dd;
  border-color: rgb(255 255 255 / 0.16);
}
.goals-toggle.active {
  color: var(--accent, #3ecf8e);
  border-color: color-mix(in srgb, var(--accent, #3ecf8e) 40%, transparent);
  transform: rotate(90deg);
}
.goals-panel {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
  margin-bottom: 0.75rem;
  padding: 0.75rem;
  border: 1px solid var(--glass-border, rgb(255 255 255 / 0.08));
  border-radius: 12px;
  background: var(--glass-bg, rgb(255 255 255 / 0.045));
  backdrop-filter: var(--glass-blur, none);
}
.goal-edit-row {
  display: grid;
  grid-template-columns: 4.5rem 6rem auto;
  align-items: center;
  gap: 0.7rem;
}
.goal-edit-name {
  font-size: 0.78rem;
  color: #cfd4dd;
  text-align: right;
}
.goal-input {
  width: 100%;
  padding: 0.3rem 0.55rem;
  border: 1px solid var(--glass-border, rgb(255 255 255 / 0.08));
  border-radius: 8px;
  background: rgb(0 0 0 / 0.25);
  color: inherit;
  font-size: 0.8rem;
  font-variant-numeric: tabular-nums;
  outline: none;
  transition: border-color 0.2s ease;
}
.goal-input:focus {
  border-color: color-mix(in srgb, var(--accent, #3ecf8e) 55%, transparent);
}
.goal-unit {
  font-size: 0.7rem;
  color: #8b93a3;
}
.goal-edit-foot {
  display: flex;
  align-items: center;
  gap: 0.6rem;
  padding-left: 5.2rem;
}
.goal-msg {
  font-size: 0.72rem;
  color: #ff8a8a;
}
.goal-msg.ok {
  color: #6fe0a8;
}
.goal-bar.done {
  background: var(--accent, #3ecf8e);
  box-shadow: 0 0 14px color-mix(in srgb, var(--accent, #3ecf8e) 75%, transparent);
}
li.done .tag-name {
  color: var(--accent, #3ecf8e);
}
.goal-check {
  margin-left: 0.35rem;
  color: var(--accent, #3ecf8e);
}
.goal-empty {
  margin: 0;
  font-size: 0.75rem;
  color: #8b93a3;
}

/* ---------------------------------------------------------------- 近 7 天回顾 */
.review-grid {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 0.75rem;
}
.review-cell {
  padding: 0.75rem 0.9rem;
  border: 1px solid var(--glass-border, rgb(255 255 255 / 0.08));
  border-radius: 12px;
  background: var(--glass-bg, rgb(255 255 255 / 0.045));
  backdrop-filter: var(--glass-blur, none);
}
.rv-label {
  margin: 0;
  font-size: 0.7rem;
  color: #8b93a3;
  letter-spacing: 0.08em;
}
.rv-value {
  margin: 0.3rem 0 0;
  font-size: 1.05rem;
  font-variant-numeric: tabular-nums;
}
.rv-value small {
  margin-left: 0.25rem;
  font-size: 0.7rem;
  color: #8b93a3;
}
.rv-sub {
  margin: 0.25rem 0 0;
  font-size: 0.68rem;
  color: #8b93a3;
}
.rv-sub.up {
  color: #6fe0a8;
}
.rv-sub.down {
  color: #ff9a8a;
}
.review-goals {
  list-style: none;
  margin: 0.75rem 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.45rem;
}
.review-goals li {
  display: grid;
  grid-template-columns: 4.5rem 1fr auto;
  align-items: center;
  gap: 0.7rem;
  font-size: 0.75rem;
  color: #cfd4dd;
}
.goal-dots {
  display: flex;
  gap: 5px;
}
.goal-dots i {
  width: 9px;
  height: 9px;
  border-radius: 50%;
  background: rgb(255 255 255 / 0.09);
  transition: background 0.3s ease, box-shadow 0.3s ease;
}
.goal-dots i.on {
  background: var(--accent, #3ecf8e);
  box-shadow: 0 0 6px color-mix(in srgb, var(--accent, #3ecf8e) 60%, transparent);
}
.rv-goal-days {
  font-size: 0.72rem;
  font-variant-numeric: tabular-nums;
  color: #8b93a3;
}
.rv-goal-days b {
  color: var(--accent, #3ecf8e);
}

/* ------------------------------------------------------------------ 科目统计 */
.tag-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.55rem;
}
.tag-list li {
  display: grid;
  grid-template-columns: 4.5rem 1fr auto auto;
  align-items: center;
  gap: 0.7rem;
  /* 错峰入场：--delay 由模板按行号给出，一行行浮起来 */
  animation: rise 0.45s var(--ease-out-expo) backwards;
  animation-delay: var(--delay, 0ms);
}

/* 换范围时的整组淡出/补位（TransitionGroup） */
.tagrow-enter-active {
  transition:
    opacity var(--t-base) var(--ease-out-expo),
    transform var(--t-base) var(--ease-morph);
}
.tagrow-leave-active {
  transition:
    opacity 0.18s ease-in,
    transform 0.18s ease-in;
  position: absolute;
  width: 100%;
}
.tagrow-move {
  transition: transform var(--t-base) var(--ease-out-expo);
}
.tagrow-enter-from {
  opacity: 0;
  transform: translateY(10px) scale(0.98);
}
.tagrow-leave-to {
  opacity: 0;
  transform: translateX(12px);
}
.row-edit {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  padding: 0;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: #8b93a3;
  cursor: pointer;
  opacity: 0;
  transition: color 0.2s ease, background 0.2s ease, opacity 0.2s ease;
}
.tag-list li:hover .row-edit {
  opacity: 1;
}
.row-edit:hover {
  color: var(--accent, #3ecf8e);
  background: rgb(255 255 255 / 0.06);
}
.rename-panel {
  margin-top: 0.75rem;
  padding: 0.7rem 0.8rem;
  border: 1px solid var(--glass-border, rgb(255 255 255 / 0.08));
  border-radius: 12px;
  background: var(--glass-bg, rgb(255 255 255 / 0.045));
  backdrop-filter: var(--glass-blur, none);
}
.rename-line {
  margin: 0 0 0.5rem;
  font-size: 0.78rem;
  color: #cfd4dd;
}
.rename-row {
  display: flex;
  align-items: center;
  gap: 0.6rem;
  flex-wrap: wrap;
}
.rename-input {
  width: 11rem;
}
.rename-hint {
  margin: 0.5rem 0 0;
  font-size: 0.68rem;
  color: #8b93a3;
}

/* -------------------------------------------------------------- 专注时段 */
.hour-peak {
  margin: 0 0 0.4rem;
  font-size: 0.78rem;
  color: #8b93a3;
}
.hour-peak b {
  color: #3ecf8e;
  font-variant-numeric: tabular-nums;
}
.tag-name {
  font-size: 0.78rem;
  color: #cfd4dd;
  text-align: right;
}
.tag-bar-track {
  display: block;
  height: 10px;
  border-radius: 999px;
  background: rgb(255 255 255 / 0.05);
  overflow: hidden;
}
.tag-bar {
  display: block;
  height: 100%;
  border-radius: 999px;
  background: var(--accent, #3ecf8e);
  box-shadow: 0 0 8px color-mix(in srgb, var(--accent, #3ecf8e) 45%, transparent);
  transition: width 0.7s var(--ease-out-expo, ease);
}
.tag-value {
  font-size: 0.78rem;
  font-variant-numeric: tabular-nums;
}
.tag-value small {
  margin-left: 0.3rem;
  font-size: 0.68rem;
  color: #8b93a3;
}

/* ------------------------------------------------------------------ 底部 */
.foot {
  display: flex;
  align-items: center;
  gap: 0.6rem;
  flex-wrap: wrap;
}
.refresh {
  align-self: flex-start;
  padding: 0.4rem 1rem;
  border: 1px solid var(--glass-border, rgb(255 255 255 / 0.08));
  border-radius: 8px;
  background: var(--glass-bg, transparent);
  color: inherit;
  cursor: pointer;
  transition: border-color 0.25s ease, transform 0.25s var(--ease-out-back, ease);
}
.refresh:hover:not(:disabled) {
  border-color: #6b7280;
  transform: translateY(-1px);
}
.refresh:active:not(:disabled) {
  transform: scale(0.97);
}
.refresh:disabled {
  opacity: 0.5;
  cursor: default;
}
@media (prefers-reduced-motion: reduce) {
  .card,
  .stats > .block,
  .tag-list li {
    animation: none;
  }
  .tagrow-enter-active,
  .tagrow-leave-active,
  .tagrow-move {
    transition: none;
  }
}
</style>
