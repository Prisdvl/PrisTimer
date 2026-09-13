<script setup lang="ts">
// ---------------------------------------------------------------------------
// 年度专注热力图（GitHub 贡献墙式）。
//
// 与月历的本质差别：**横向展开一整年**，把"这一年有没有坚持"变成一眼可见的
// 形状 —— 断档的空白区比任何数字都刺眼。列 = 周，行 = 周一…周日，
// 共 53 列（一年最多跨 53 个周对）。
//
// 月份时长条单独放在下方：热力图回答"什么时候学"，月条回答"每个月攒了多少"，
// 两者位置对齐（月条按月份顺序、热力图按月推进），同一块区域里互相印证。
// ---------------------------------------------------------------------------

import { computed, onMounted, onUnmounted, ref } from "vue";
import type { DailyStat } from "../api";
import { addDays, formatDuration, heatLevel, toDayKey } from "../date";

const props = defineProps<{
  /** 覆盖窗口内逐日数据（含空白天不必补，缺省即 0）。 */
  stats: DailyStat[];
  /** 窗口结束日（本地 `YYYY-MM-DD`）。 */
  today: string;
  /** 窗口长度（天）。365 表示"最近一年"。 */
  days: number;
}>();

const WEEKDAYS = ["一", "二", "三", "四", "五", "六", "日"];

/** 只标周一/三/五，7 行全标会挤成一列数字。 */
const LABELLED_ROWS = [0, 2, 4];

function parseDay(key: string): Date {
  const [y, m, d] = key.split("-").map(Number);
  return new Date(y, m - 1, d);
}

const byDay = computed(() => {
  const map = new Map<string, DailyStat>();
  for (const stat of props.stats) map.set(stat.day, stat);
  return map;
});

/** 起点：结束日往回 `days - 1` 天，再对齐到所在周的周一。 */
const startAligned = computed(() => {
  const end = parseDay(props.today);
  const raw = addDays(end, -(props.days - 1));
  return addDays(raw, -((raw.getDay() + 6) % 7));
});

const endDate = computed(() => parseDay(props.today));

interface Tile {
  key: string;
  level: number;
  monthKey: string;
  title: string;
  /** 落在统计窗口之外（对齐用的补位格）。 */
  outside: boolean;
  /** 今天之后的日期。 */
  future: boolean;
}

const weeks = computed<Tile[][]>(() => {
  const start = startAligned.value;
  const end = endDate.value;
  const firstKey = toDayKey(addDays(end, -(props.days - 1)));
  const todayKey = props.today;
  const spanDays = Math.round((end.getTime() - start.getTime()) / 86_400_000) + 1;
  const weekCount = Math.ceil(spanDays / 7);

  const grid: Tile[][] = [];
  for (let week = 0; week < weekCount; week += 1) {
    const column: Tile[] = [];
    for (let row = 0; row < 7; row += 1) {
      const date = addDays(start, week * 7 + row);
      const key = toDayKey(date);
      const stat = byDay.value.get(key);
      column.push({
        key,
        level: heatLevel(stat?.totalMs ?? 0),
        monthKey: key.slice(0, 7),
        title: stat
          ? `${key} · ${formatDuration(stat.totalMs)} · ${stat.sessionCount} 次`
          : `${key} · 没有记录`,
        outside: key < firstKey || key > todayKey,
        future: key > todayKey,
      });
    }
    grid.push(column);
  }
  return grid;
});

/** 每列顶部标月份：只在"该列出现了新的月份"时打标签，避免重复。 */
const monthLabels = computed(() =>
  weeks.value.map((column, index) => {
    const inRange = column.find((tile) => !tile.outside);
    if (!inRange) return null;
    const month = Number(inRange.monthKey.slice(5));
    if (index === 0) return { label: `${month}月`, index };
    const prev = weeks.value[index - 1]?.find((tile) => !tile.outside);
    const prevMonth = prev ? Number(prev.monthKey.slice(5)) : -1;
    return prevMonth === month ? null : { label: `${month}月`, index };
  }),
);

/** 按月聚合的专注时长，覆盖窗口内的每一个自然月（含 0 记录的月）。
 *  窗口跨 12–13 个月，首尾可能同名（如两个 9 月），需要年份消歧。 */
const months = computed(() => {
  const byMonth = new Map<string, number>();
  for (const stat of props.stats) {
    const key = stat.day.slice(0, 7);
    byMonth.set(key, (byMonth.get(key) ?? 0) + stat.totalMs);
  }
  const out: Array<{ key: string; label: string; ms: number }> = [];
  const cursor = new Date(startAligned.value.getFullYear(), startAligned.value.getMonth(), 1);
  const lastKey = props.today.slice(0, 7);
  // 上限 14 个月，纯防御：万一 days 传得离谱也不会无限循环
  for (let guard = 0; guard < 14; guard += 1) {
    const key = `${cursor.getFullYear()}-${String(cursor.getMonth() + 1).padStart(2, "0")}`;
    const month = cursor.getMonth() + 1;
    // 首月和跨年的 1 月带上年份，其余只写月号 —— 避免出现两个「9月」分不清
    const withYear = out.length === 0 || month === 1;
    out.push({
      key,
      label: withYear ? `${cursor.getFullYear() % 100}年${month}月` : `${month}月`,
      ms: byMonth.get(key) ?? 0,
    });
    if (key >= lastKey) break;
    cursor.setMonth(cursor.getMonth() + 1);
  }
  return out;
});

const yearTotalMs = computed(() => months.value.reduce((sum, m) => sum + m.ms, 0));

/** 每个月在墙面上占据的列区间 [start, end)。
 *  列归属按该列周一（首格）的月份算 —— 与顶部月份刻度、下方月份条共用同一套
 *  列几何，三者逐列对齐。跨两周的月（如月初在周三）自然多占一格。 */
const monthSpans = computed(() => {
  const colMonth = weeks.value.map((column) => column.find((tile) => !tile.outside)?.monthKey ?? null);
  const spans: Array<{ key: string; start: number; end: number }> = [];
  for (let ci = 0; ci < colMonth.length; ci += 1) {
    const mk = colMonth[ci];
    if (!mk) continue;
    const last = spans[spans.length - 1];
    if (last && last.key === mk) last.end = ci + 1;
    else spans.push({ key: mk, start: ci, end: ci + 1 });
  }

  // 收尾那个月往往只剩一两列（窗口到今天为止）：光内边距就 8px，
  // 两行文字会被挤成一条空缝 —— 偏偏"本月"是最该看清的那一格。
  // 向右补到至少 3 列：它右边本来就是空的，延伸出去不会撞到别的月卡。
  // ★ 但必须以总列数为上限：gridColumn 越过列数会在隐式网格里多生成列，
  //   自适应把 53 列铺满容器后，这多出的两列就是唯一的横向溢出源。
  //   补不到 3 列时（尾月已贴墙末）就地截断，窄卡文案由 monthValue 按像素宽降级。
  const MIN_SPAN = 3;
  const tail = spans[spans.length - 1];
  if (tail && tail.end - tail.start < MIN_SPAN) {
    tail.end = Math.min(tail.start + MIN_SPAN, weeks.value.length);
  }

  return spans;
});

/** key → 月份条文案（月名 + 时长），供 monthSpans 渲染时查表。 */
const monthInfo = computed(() => {
  const map = new Map<string, { label: string; ms: number }>();
  for (const m of months.value) map.set(m.key, { label: m.label, ms: m.ms });
  return map;
});

/** 悬停某个月：其余月份的格子淡出，把 "这个月" 从一年里拎出来。 */
const activeMonth = ref<string | null>(null);

const LEVELS = [0, 1, 2, 3, 4];

function fmtMonth(ms: number): string {
  if (ms <= 0) return "—";
  const hours = ms / 3_600_000;
  return hours >= 10 ? `${Math.round(hours)} 小时` : `${hours.toFixed(1)} 小时`;
}

/** 窄卡专用：数字 + h。同字号下约 19px，塞得进 24px 内容区，且保住单位。 */
function fmtMonthShort(ms: number): string {
  if (ms <= 0) return "—";
  const hours = ms / 3_600_000;
  return `${hours >= 10 ? Math.round(hours) : hours.toFixed(1)}h`;
}

// ---------------------------------------------------------------------------
// 自适应列宽：格子尺寸不再写死 10px，而是按容器实际宽度算出来
// （GitHub 墙铺满可用宽度，宽窗口格子变大、窄窗口变小），下限 6px、上限 24px。
// ★ 上限不能太保守：最大化（2551 物理 / ~1700 逻辑宽）时可用宽 ~1500px，
//   53 列摊下来格子能到 26px+ —— 上限 15px 会让墙只有 900px 宽，右边空一大条。
// 模板里所有几何都引用 --cell 变量，月份条与顶部刻度自动跟随。
// ---------------------------------------------------------------------------

/** 列间隙，与 CSS 的 gap 同步。 */
const GAP = 2;
const scrollEl = ref<HTMLElement | null>(null);
const cellSize = ref(10);

function recalc(): void {
  const sc = scrollEl.value;
  if (!sc) return;
  const n = weeks.value.length || 53;
  // 可用宽度 = 容器 − .wall 左右 padding(0.9rem×2≈29) 与边框 2 − 星期槽 20
  const available = sc.clientWidth - 31 - 20;
  const size = Math.floor((available - (n - 1) * GAP) / n);
  cellSize.value = Math.max(6, Math.min(24, size));
}

let resizeObserver: ResizeObserver | null = null;
onMounted(() => {
  recalc();
  resizeObserver = new ResizeObserver(recalc);
  if (scrollEl.value) resizeObserver.observe(scrollEl.value);
});
onUnmounted(() => resizeObserver?.disconnect());

/** 按卡片实际像素宽选文案：宽卡给完整「12.5 小时」，窄卡给「12.5h」。 */
function monthValue(span: { key: string; start: number; end: number }): string {
  const ms = monthInfo.value.get(span.key)?.ms ?? 0;
  const width = (span.end - span.start) * (cellSize.value + GAP) - GAP;
  return width < 56 ? fmtMonthShort(ms) : fmtMonth(ms);
}
</script>

<template>
  <section class="year">
    <p class="summary">
      近一年共专注 <b>{{ formatDuration(yearTotalMs) }}</b>
      <span class="gap">·</span>
      有记录 {{ stats.length }} 天
    </p>

    <div ref="scrollEl" class="scroll">
      <div class="wall" :style="{ '--cell': `${cellSize}px` }">
        <!-- 月份刻度：与下方列一一对应 -->
        <div class="months">
          <span class="gutter" />
          <span
            v-for="(label, index) in monthLabels"
            :key="index"
            class="month-tick"
            :class="{ show: label !== null }"
          >
            {{ label?.label ?? "" }}
          </span>
        </div>

        <div class="body">
          <div class="weekdays">
            <span v-for="(name, row) in WEEKDAYS" :key="name">
              {{ LABELLED_ROWS.includes(row) ? name : "" }}
            </span>
          </div>
          <div class="columns">
            <div v-for="(column, ci) in weeks" :key="ci" class="column">
              <span
                v-for="tile in column"
                :key="tile.key"
                class="tile"
                :class="[
                  `level-${tile.level}`,
                  {
                    outside: tile.outside,
                    future: tile.future,
                    dim: activeMonth !== null && tile.monthKey !== activeMonth,
                  },
                ]"
                :title="tile.title"
              />
            </div>
          </div>
        </div>

        <!-- 每月时长条：与热力图共用同一套 53 列几何，每个月的卡片精确压在
             自己的列区间下方（跨几周就横跨几格），鼠标划过同时高亮墙上对应列 -->
        <ul class="month-strip" :class="{ focusing: activeMonth !== null }">
          <li
            v-for="span in monthSpans"
            :key="span.key"
            :class="{ on: activeMonth === span.key, empty: (monthInfo.get(span.key)?.ms ?? 0) <= 0 }"
            :style="{ gridColumn: `${span.start + 1} / ${span.end + 1}` }"
            :title="`${monthInfo.get(span.key)?.label ?? span.key} · ${fmtMonth(monthInfo.get(span.key)?.ms ?? 0)}`"
            @mouseenter="activeMonth = span.key"
            @mouseleave="activeMonth = null"
          >
            <span class="m-name">{{ monthInfo.get(span.key)?.label ?? span.key }}</span>
            <span class="m-value">{{ monthValue(span) }}</span>
          </li>
        </ul>
      </div>
    </div>

    <footer class="legend">
      <span>少</span>
      <i v-for="level in LEVELS" :key="level" :class="`level-${level}`" />
      <span>多</span>
      <span class="hint">一格 = 一天 · 悬停月份可高亮</span>
    </footer>
  </section>
</template>

<style scoped>
.year {
  display: flex;
  flex-direction: column;
  gap: 0.7rem;
}

.summary {
  margin: 0;
  font-size: 0.78rem;
  color: var(--ink-dim);
}
.summary b {
  color: var(--ui-accent);
  font-variant-numeric: tabular-nums;
}
.summary .gap {
  margin: 0 0.3rem;
  color: var(--ink-faint);
}

/* 窄窗口时允许横向滚动，不让热力图被压扁变形 */
.scroll {
  overflow-x: auto;
  padding-bottom: 2px;
}
.wall {
  display: inline-flex;
  flex-direction: column;
  gap: 3px;
  padding: 0.75rem 0.9rem 0.85rem;
  background: var(--glass-bg);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius);
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-edge), var(--glass-shadow);
}

.months {
  display: flex;
  gap: 2px;
  height: 12px;
}
.gutter {
  /* 18px + 行内 gap 2px = 20px，与 .body 的星期槽(16)+gap(4) 对齐，
     让顶部月份刻度也精确压在列上 */
  width: 18px;
  flex: none;
}
.month-tick {
  width: var(--cell, 10px);
  flex: none;
  font-size: 0.56rem;
  line-height: 12px;
  color: transparent;
  white-space: nowrap;
  overflow: visible;
}
/* 只在有标签的列上写字；文字自然向右溢出，不会撑宽列宽 */
.month-tick.show {
  color: var(--ink-faint);
}

.body {
  display: flex;
  gap: 4px;
}
.weekdays {
  display: grid;
  grid-template-rows: repeat(7, var(--cell, 10px));
  gap: 2px;
  width: 16px;
  flex: none;
}
.weekdays span {
  font-size: 0.55rem;
  line-height: 10px;
  color: var(--ink-faint);
  text-align: left;
}

.columns {
  display: flex;
  gap: 2px;
}
.column {
  display: grid;
  grid-template-rows: repeat(7, var(--cell, 10px));
  gap: 2px;
}

.tile {
  width: var(--cell, 10px);
  height: var(--cell, 10px);
  border-radius: 2.5px;
  transition:
    transform var(--t-base) var(--ease-out-back),
    box-shadow var(--t-base) ease,
    opacity var(--t-base) var(--ease-out-expo),
    background var(--t-base) ease;
  cursor: default;
}
.tile:hover {
  transform: scale(1.5);
  box-shadow: 0 2px 10px rgb(0 0 0 / 0.5);
  position: relative;
  z-index: 2;
}
.tile.outside {
  opacity: 0.22;
}
.tile.future {
  opacity: 0.3;
}
/* 悬停月份时的"聚焦"效果 */
.tile.dim {
  opacity: 0.16;
}

.level-0 {
  background: color-mix(in srgb, var(--ink) 5.5%, transparent);
}
.level-1 {
  background: #1f4d3a;
}
.level-2 {
  background: #24704f;
}
.level-3 {
  background: #2b9c67;
}
.level-4 {
  background: var(--ui-accent, #3ecf8e);
  box-shadow: 0 0 6px color-mix(in srgb, var(--ui-accent) 35%, transparent);
}

/* ---------------------------------------------------------- 每月时长条 */
/* 与热力图逐列对齐：左侧 20px = 星期槽 16px + .body 的 gap 4px。
   每个 li 通过内联 grid-column 跨到自己的列区间，跨几周就横跨几格。

   ★ 列数必须**跟着 weeks.length 走，不能写死**。
   原来写的是 `repeat(53, 10px)`，而真实列数是
   `Math.ceil(对齐后的天数 / 7)`：窗口 365 天再对齐到周一，落在 53 或 **54**
   列都正常。一旦是 54，最后一个月的卡片就落进 **隐式列** —— 隐式列没有
   显式轨道，宽度由内容撑开（约 26px 而不是 10px），整条月带于是比上面的
   方格墙宽出二十几个像素：`.scroll` 冒出横向滚动条，最后一张卡被挤得与列错开。
   用 `grid-auto-flow: column` + `grid-auto-columns` 让每一列都是 var(--cell)，
   未用到的列不占宽，总宽就恒等于「列数 × (cell+2) − 2」与方格墙严丝合缝。 */
.month-strip {
  display: grid;
  grid-auto-flow: column;
  grid-auto-columns: var(--cell, 10px);
  gap: 2px;
  margin: 0;
  padding: 0 0 2px 20px;
  list-style: none;
}
.month-strip li {
  grid-row: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding: 3px 4px 4px;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-sm);
  background: var(--glass-bg);
  backdrop-filter: var(--glass-blur);
  cursor: default;
  overflow: hidden;
  transition:
    border-color var(--t-base) var(--ease-out-expo),
    background var(--t-base) var(--ease-out-expo),
    transform var(--t-base) var(--ease-out-back),
    opacity var(--t-base) ease;
}
.month-strip li:hover {
  transform: translateY(-2px);
}
/* 悬停聚焦：其它月份整卡淡出（墙上对应列也在同步淡出） */
.month-strip.focusing li:not(.on) {
  opacity: 0.3;
}
.month-strip li.on {
  border-color: var(--ui-accent);
  background: color-mix(in srgb, var(--ui-accent) 12%, transparent);
  box-shadow: 0 0 14px color-mix(in srgb, var(--ui-accent) 20%, transparent);
}
.month-strip li.empty {
  opacity: 0.55;
}
.month-strip.focusing li.on.empty {
  opacity: 1;
}
.m-name {
  font-size: 0.55rem;
  line-height: 1.25;
  color: var(--ink-faint);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.m-value {
  font-size: 0.6rem;
  line-height: 1.25;
  color: var(--ink-soft);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  transition: color var(--t-base) ease;
}
.month-strip li.on .m-value {
  color: var(--ui-accent);
}

/* ---------------------------------------------------------------- 图例 */
.legend {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 4px;
  font-size: 0.68rem;
  color: var(--ink-dim);
}
.legend i {
  display: inline-block;
  width: 11px;
  height: 11px;
  border-radius: 2.5px;
  transition: background var(--t-base) ease;
}
.legend .hint {
  margin-left: auto;
  margin-right: 0.3rem;
  color: var(--ink-faint);
}
</style>
