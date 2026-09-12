<script setup lang="ts">
// ---------------------------------------------------------------------------
// 趋势图：同一份日频数据，柱状 / 折线两种形态，切换带过渡动画。
//
// 动画结构（柱 → 折线）：
//   退场  Vue <Transition mode="out-in"> 的 leave 阶段 —— 柱体按 --i 交错
//         scaleY 收缩（transform-box: fill-box 让每根柱以自身底边为基准）；
//   进场  新 <g> 挂载后触发普通 CSS 挂载动画 —— 折线用 pathLength="1"
//         归一化后从 dashoffset 1 描到 0，面积渐显、圆点交错弹入。
// 反向（折线 → 柱）同理：折线整体淡出，柱体交错长回来。
// ---------------------------------------------------------------------------

import { computed } from "vue";
import { formatDuration } from "../date";

export interface TrendRow {
  day: string;
  totalMs: number;
  label: string;
  showLabel: boolean;
}

const props = defineProps<{
  rows: TrendRow[];
  mode: "bar" | "line";
  today: string;
}>();

const W = 640;
const H = 180;
const PAD_X = 10;
const PAD_B = 26;
const PAD_T = 14;
const BASE = H - PAD_B;
const INNER_H = H - PAD_B - PAD_T;

const maxMs = computed(() =>
  Math.max(60_000, ...props.rows.map((row) => row.totalMs)),
);

const slot = computed(() => (W - PAD_X * 2) / Math.max(1, props.rows.length));

/** 柱体细：宽度封顶 10px，随槽位收缩但不小于 3px。 */
const barW = computed(() => Math.min(10, Math.max(3, slot.value * 0.45)));

/** 交错节奏随数量调整：90 根柱时不能再用 7 根柱的间隔。 */
const stepMs = computed(() =>
  Math.max(3, Math.min(14, Math.round(600 / Math.max(1, props.rows.length)))),
);

interface Pt {
  x: number;
  y: number;
}

const points = computed<Pt[]>(() =>
  props.rows.map((row, i) => ({
    x: round(PAD_X + slot.value * i + slot.value / 2),
    y: round(BASE - (row.totalMs / maxMs.value) * INNER_H),
  })),
);

/** Catmull-Rom → 三次贝塞尔：折线平滑，不出现锐角拐点。 */
const linePath = computed(() => smooth(points.value));

const areaPath = computed(() => {
  const pts = points.value;
  if (pts.length < 2) {
    return "";
  }
  const last = pts[pts.length - 1];
  const first = pts[0];
  return `${linePath.value} L ${last.x} ${BASE} L ${first.x} ${BASE} Z`;
});

const labels = computed(() =>
  props.rows
    .map((row, i) => ({ ...row, x: round(PAD_X + slot.value * i + slot.value / 2) }))
    .filter((row) => row.showLabel),
);

const round = (n: number): number => Math.round(n * 10) / 10;

function smooth(pts: Pt[]): string {
  if (pts.length === 0) {
    return "";
  }
  if (pts.length === 1) {
    return `M ${pts[0].x} ${pts[0].y}`;
  }
  let d = `M ${pts[0].x} ${pts[0].y}`;
  for (let i = 0; i < pts.length - 1; i += 1) {
    const p0 = pts[Math.max(0, i - 1)];
    const p1 = pts[i];
    const p2 = pts[i + 1];
    const p3 = pts[Math.min(pts.length - 1, i + 2)];
    const c1x = p1.x + (p2.x - p0.x) / 6;
    const c1y = p1.y + (p2.y - p0.y) / 6;
    const c2x = p2.x - (p3.x - p1.x) / 6;
    const c2y = p2.y - (p3.y - p1.y) / 6;
    d += ` C ${round(c1x)} ${round(c1y)}, ${round(c2x)} ${round(c2y)}, ${p2.x} ${p2.y}`;
  }
  return d;
}
</script>

<template>
  <svg
    class="chart"
    :viewBox="`0 0 ${W} ${H}`"
    preserveAspectRatio="xMidYMid meet"
    role="img"
  >
    <defs>
      <linearGradient id="trend-area" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#3ecf8e" stop-opacity="0.22" />
        <stop offset="1" stop-color="#3ecf8e" stop-opacity="0" />
      </linearGradient>
    </defs>

    <Transition name="chart" mode="out-in">
      <g v-if="mode === 'bar'" key="bar" :style="{ '--step': `${stepMs}ms` }">
        <rect
          v-for="(row, i) in rows"
          :key="row.day"
          class="bar"
          :class="{ today: row.day === today, zero: row.totalMs === 0 }"
          :style="{ '--i': i }"
          :x="PAD_X + slot * i + (slot - barW) / 2"
          :y="BASE - Math.max(row.totalMs === 0 ? 2 : 3, (row.totalMs / maxMs) * INNER_H)"
          :width="barW"
          :height="Math.max(row.totalMs === 0 ? 2 : 3, (row.totalMs / maxMs) * INNER_H)"
          :rx="barW / 2"
        >
          <title>{{ row.day }} · {{ formatDuration(row.totalMs) }}</title>
        </rect>
      </g>

      <g v-else key="line" :style="{ '--step': `${stepMs}ms` }">
        <path class="line-area" :d="areaPath" fill="url(#trend-area)" />
        <path
          class="line-path"
          :d="linePath"
          pathLength="1"
          fill="none"
          stroke="#3ecf8e"
          stroke-width="2.5"
          stroke-linecap="round"
        />
        <circle
          v-for="(p, i) in points"
          :key="rows[i].day"
          class="dot"
          :class="{ today: rows[i].day === today }"
          :style="{ '--i': i }"
          :cx="p.x"
          :cy="p.y"
          :r="rows[i].day === today ? 4.5 : 2.5"
        >
          <title>{{ rows[i].day }} · {{ formatDuration(rows[i].totalMs) }}</title>
        </circle>
      </g>
    </Transition>

    <text
      v-for="label in labels"
      :key="`label-${label.day}`"
      class="axis-label"
      :x="label.x"
      :y="H - 8"
      text-anchor="middle"
    >
      {{ label.label }}
    </text>
  </svg>
</template>

<style scoped>
.chart {
  width: 100%;
  height: auto;
  display: block;
}

/* ------------------------------------------------------------------ 柱状 */
.bar {
  fill: rgb(62 207 142 / 0.45);
  transform-box: fill-box;
  transform-origin: 50% 100%;
  animation: bar-in 0.5s var(--ease-out-expo, cubic-bezier(0.16, 1, 0.3, 1)) backwards;
  animation-delay: calc(var(--i) * var(--step, 10ms));
  transition: fill 0.2s ease, x 0.45s var(--ease-out-expo, ease), y 0.4s var(--ease-out-expo, ease), width 0.45s var(--ease-out-expo, ease), height 0.4s var(--ease-out-expo, ease);
}
.bar:hover {
  fill: #3ecf8e;
}
.bar.today {
  fill: #3ecf8e;
}
.bar.zero {
  fill: rgb(255 255 255 / 0.08);
}
@keyframes bar-in {
  from {
    transform: scaleY(0);
  }
}

/* ------------------------------------------------------------------ 折线 */
/* pathLength="1" 把描线归一化：dasharray 1 + dashoffset 1→0 即描线动画 */
.line-path {
  stroke-dasharray: 1;
  animation: draw 0.7s var(--ease-out-expo, cubic-bezier(0.16, 1, 0.3, 1)) 0.05s backwards;
}
@keyframes draw {
  from {
    stroke-dashoffset: 1;
  }
}
.line-area {
  animation: fade 0.5s ease 0.45s backwards;
}
.dot {
  fill: #3ecf8e;
  transform-box: fill-box;
  transform-origin: center;
  animation: pop 0.35s var(--ease-out-back, ease) backwards;
  animation-delay: calc(var(--i) * var(--step, 10ms) + 0.25s);
  transition: fill 0.2s ease, stroke-width 0.2s ease, cx 0.45s var(--ease-out-expo, ease), cy 0.45s var(--ease-out-expo, ease);
}
.dot.today {
  stroke: rgb(62 207 142 / 0.35);
  stroke-width: 3;
}
@keyframes pop {
  from {
    transform: scale(0);
  }
}
@keyframes fade {
  from {
    opacity: 0;
  }
}

/* ------------------------------------------------------------------ 切换 */
/* 退场：整体淡出兜底（Vue 依根元素的 transition 计算时长），
   柱体再各自交错收缩，折线子元素直接跟随整体淡出 */
.chart-leave-active {
  transition: opacity 0.4s ease;
  opacity: 0;
}
.chart-leave-active .bar {
  animation: bar-out 0.22s ease-in forwards;
  animation-delay: calc(var(--i) * var(--step, 10ms));
}
@keyframes bar-out {
  to {
    transform: scaleY(0);
    opacity: 0;
  }
}

/* ------------------------------------------------------------------ 坐标轴 */
.axis-label {
  fill: #6b7280;
  font-size: 10px;
  font-variant-numeric: tabular-nums;
}

@media (prefers-reduced-motion: reduce) {
  .bar,
  .line-path,
  .line-area,
  .dot {
    animation: none;
  }
}
</style>
