<script setup lang="ts">
// ---------------------------------------------------------------------------
// 悬浮信息窗：贴托盘旁的小窗口，**窗口最小化时持续展示**蓝牙与额度信息。
//
// 由 `insight::commands::overlay_toggle` 控制显隐（Rust 侧负责定位到托盘旁），
// 本组件只负责在给定的窗口里把信息渲染出来。
//
// 与主窗口状态栏共用 `useInsight`：各自窗口的 JS 上下文各自订阅
// `insight:update`（Tauri 事件会广播到所有窗口），数据永远同步。
// ---------------------------------------------------------------------------

import { computed } from "vue";
import { useInsight, batteryText, kindLabel } from "../composables/useInsight";

const { snapshot, deviceLine, quotaLine } = useInsight();

const btLine = computed(() => deviceLine(snapshot.value.bluetooth));
const qtLine = computed(() => quotaLine(snapshot.value.quota));

/** 多窗口额度（opencode Go 的三个重置窗口）。空数组 = 单值接口。 */
const quotaWindows = computed(() =>
  snapshot.value.quota.status === "ok" ? snapshot.value.quota.windows ?? [] : [],
);

/** 窗口重置时刻的人话（解析失败原样显示）。 */
function resetText(iso: string | null): string {
  if (!iso) return "重置未知";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return `${date.toLocaleString("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" })} 重置`;
}

/** 蓝牙设备多行罗列（浮窗空间比状态栏宽裕，一台一行更好读）。 */
const btLines = computed(() => {
  const bt = snapshot.value.bluetooth;
  if (bt.status !== "ok" || bt.devices.length === 0) return [];
  return bt.devices.map((d) => ({
    name: d.name,
    type: kindLabel(d.kind),
    battery: batteryText(d.batteryPercent),
    unknown: d.batteryPercent === null,
  }));
});
</script>

<template>
  <div class="overlay">
    <!-- 蓝牙 -->
    <div class="row" :class="snapshot.bluetooth.status">
      <svg class="ic" viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="m7 7 10 10-5 5V2l5 5L7 17" />
      </svg>
      <div class="cols">
        <p class="line" :title="btLine">{{ btLine }}</p>
        <p v-for="d in btLines" :key="d.name" class="sub">
          <span class="nm">{{ d.name }}</span>
          <span class="ty">{{ d.type }}</span>
          <span class="bat" :class="{ unknown: d.unknown }">{{ d.battery }}</span>
        </p>
      </div>
    </div>
    <!-- 额度 -->
    <div class="row" :class="snapshot.quota.status">
      <svg class="ic" viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <rect width="20" height="14" x="2" y="5" rx="2" />
        <path d="M2 10h20" />
      </svg>
      <div class="cols">
        <!-- 多窗口时逐档罗列（带重置时间）；单值接口退回一句话 -->
        <template v-if="quotaWindows.length">
          <p
            v-for="win in quotaWindows"
            :key="win.label"
            class="sub"
          >
            <span class="nm">{{ win.label }}</span>
            <span class="bat" :class="{ unknown: win.remainingPercent <= 0, low: win.remainingPercent < 20 }">
              {{ Math.round(win.remainingPercent) }}%
            </span>
            <span class="ty">{{ resetText(win.resetsAt) }}</span>
          </p>
        </template>
        <p v-else class="line" :title="qtLine">{{ qtLine }}</p>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 整窗即卡片：透明窗口，玻璃圆角自己画 */
.overlay {
  position: fixed;
  inset: 0;
  display: flex;
  flex-direction: column;
  gap: 0.45rem;
  padding: 0.6rem 0.7rem;
  border: 1px solid var(--glass-border);
  border-radius: 14px;
  background: color-mix(in srgb, var(--glass-bg-deep) 92%, transparent);
  backdrop-filter: blur(20px) saturate(1.4);
  box-shadow: 0 10px 28px rgb(0 0 0 / 0.4);
  color: var(--ink-soft);
  font-size: 0.76rem;
  user-select: none;
}
.row {
  display: flex;
  align-items: flex-start;
  gap: 0.4rem;
  min-width: 0;
}
.ic {
  flex: none;
  margin-top: 2px;
  color: var(--ink-faint);
}
/* 状态色跟随 Rust 侧语气 */
.row.ok .ic {
  color: var(--accent);
}
.row.warn .ic {
  color: #e8b64c;
}
.row.err .ic {
  color: #ff8a8a;
}
.cols {
  display: flex;
  flex-direction: column;
  gap: 0.14rem;
  min-width: 0;
}
.line {
  margin: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--ink-soft);
}
.sub {
  display: flex;
  gap: 0.4rem;
  margin: 0;
  font-size: 0.68rem;
  color: var(--ink-dim);
  overflow: hidden;
}
.sub .nm {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--ink-soft);
}
.sub .ty {
  flex: none;
  font-size: 0.6rem;
  padding: 0 0.2rem;
  border: 1px solid var(--glass-border);
  border-radius: 4px;
  color: var(--ink-faint);
}
.sub .bat {
  flex: none;
  font-variant-numeric: tabular-nums;
  color: var(--accent);
}
.sub .bat.unknown {
  color: var(--ink-faint);
}
/* 剩余低于 20% 的配额窗口标黄 */
.sub .bat.low {
  color: #e8b64c;
}
</style>

<style>
/* 覆盖全局的 <html> 背景（glass.css 给 html 画了渐变底色）：
   悬浮窗是透明窗口，页面背景必须透明，玻璃卡片才有意义。
   这个选择器只会在 overlay 窗口命中（main.ts 挂载前加了这个类）。 */
html.overlay-mode {
  background: transparent !important;
}
</style>