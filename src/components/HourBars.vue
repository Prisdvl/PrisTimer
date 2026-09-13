<script setup lang="ts">
// ---------------------------------------------------------------------------
// 专注时段分布：24 根柱，一根代表一天里的一个钟点。
//
// 口径（与 store 层 hour_totals_between 一致）：会话按**开始时刻**的本地
// 小时归属，13:50–14:40 整体算 13 点 —— 回答的是「通常在哪个钟点坐下开始学」。
// 数据形状由父组件补全成固定 24 行（没记录的小时给 0），这里只管几何与动效。
// ---------------------------------------------------------------------------

import { computed } from "vue";
import { formatDuration } from "../date";

export interface HourRow {
  /** 0–23。 */
  hour: number;
  totalMs: number;
  sessionCount: number;
}

const props = defineProps<{ rows: HourRow[] }>();

const W = 640;
const H = 170;
const PAD_X = 8;
const PAD_B = 24;
const PAD_T = 10;
const BASE = H - PAD_B;
const INNER_H = H - PAD_B - PAD_T;

const slot = (W - PAD_X * 2) / 24;
/** 24 根柱的可用宽度比 90 根宽裕得多，柱体可以粗一些。 */
const barW = Math.min(18, slot * 0.55);

/** 归一化基准兜底 1 分钟：全空时柱体不因除零爆掉（父组件此时不渲染本组件）。 */
const maxMs = computed(() => Math.max(60_000, ...props.rows.map((r) => r.totalMs)));

/** 峰值小时：时长最长的那格（并列取最早），全空为 -1。 */
const peakIndex = computed(() => {
  let best = -1;
  props.rows.forEach((row, i) => {
    if (row.totalMs > 0 && (best < 0 || row.totalMs > props.rows[best].totalMs)) {
      best = i;
    }
  });
  return best;
});

/** 悬停提示：`14:00–15:00 · 45 分钟 · 2 次`。 */
function tooltip(row: HourRow): string {
  const hh = (h: number) => String(h % 24).padStart(2, "0");
  return `${hh(row.hour)}:00–${hh(row.hour + 1)}:00 · ${formatDuration(row.totalMs)} · ${row.sessionCount} 次`;
}

/** 横轴只标 5 个刻度，24 根柱全标会挤成一团。 */
const tickHours = [0, 6, 12, 18, 23];

const stepMs = 9;
</script>

<template>
  <svg
    class="hours"
    :viewBox="`0 0 ${W} ${H}`"
    preserveAspectRatio="xMidYMid meet"
    role="img"
  >
    <g :style="{ '--step': `${stepMs}ms` }">
      <rect
        v-for="(row, i) in rows"
        :key="row.hour"
        class="hbar"
        :class="{ peak: i === peakIndex, zero: row.totalMs === 0 }"
        :style="{ '--i': i }"
        :x="PAD_X + slot * i + (slot - barW) / 2"
        :y="BASE - Math.max(row.totalMs === 0 ? 2 : 3, (row.totalMs / maxMs) * INNER_H)"
        :width="barW"
        :height="Math.max(row.totalMs === 0 ? 2 : 3, (row.totalMs / maxMs) * INNER_H)"
        :rx="barW / 2"
      >
        <title>{{ tooltip(row) }}</title>
      </rect>
    </g>

    <text
      v-for="h in tickHours"
      :key="`tick-${h}`"
      class="axis-label"
      :class="{ midnight: h === 0 || h === 23 }"
      :x="PAD_X + slot * h + slot / 2"
      :y="H - 8"
      text-anchor="middle"
    >
      {{ h }}时
    </text>
  </svg>
</template>

<style scoped>
.hours {
  width: 100%;
  height: auto;
  display: block;
}

.hbar {
  fill: color-mix(in srgb, var(--ui-accent) 40%, transparent);
  transform-box: fill-box;
  transform-origin: 50% 100%;
  animation: hbar-in 0.5s var(--ease-out-expo, cubic-bezier(0.16, 1, 0.3, 1)) backwards;
  animation-delay: calc(var(--i) * var(--step, 9ms));
  transition: fill 0.2s ease;
}
/* 峰值那根用实心 + 轻微描边光晕，一眼锁定「我的黄金时段」 */
.hbar.peak {
  fill: var(--ui-accent);
  filter: drop-shadow(0 0 4px color-mix(in srgb, var(--ui-accent) 45%, transparent));
}
.hbar:hover:not(.zero) {
  fill: var(--ui-accent);
}
.hbar.zero {
  fill: color-mix(in srgb, var(--ink) 8%, transparent);
}
@keyframes hbar-in {
  from {
    transform: scaleY(0);
  }
}

.axis-label {
  fill: var(--ink-faint);
  font-size: 10px;
  font-variant-numeric: tabular-nums;
}
.axis-label.midnight {
  fill: var(--ink-dim);
}

@media (prefers-reduced-motion: reduce) {
  .hbar {
    animation: none;
  }
}
</style>
