<script setup lang="ts">
// ---------------------------------------------------------------------------
// 迷你组件：整窗缩成一枚贴边置顶的"组件"，只留时间与开始/还原/托盘三钮。
// 整面可拖拽（data-tauri-drag-region），按钮不拖拽只响应点击；
// 底部细线是倒计时进度（与主表盘的外圈细弧同源）。
//
// P1-⑤ 第二步自 App.vue 拆出：显示状态走 props，开始/暂停直接调
// timerApi（与拆出前同一 IPC），形态切换走事件回到 App（几何归
// useMiniWindow 管）。
// ---------------------------------------------------------------------------

import { timerApi } from "../api";

defineProps<{
  display: string;
  miniLabel: string;
  selectedTag: string | null;
  isRunning: boolean;
  /** 倒计时进度（0..1），无目标时为 null（细线隐藏）。 */
  progress: number | null;
}>();

const emit = defineEmits<{ toggle: []; tray: [] }>();
</script>

<template>
  <div class="mini" data-tauri-drag-region>
    <span
      v-if="progress !== null"
      class="mini-progress"
      :style="{ transform: `scaleX(${progress})` }"
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
      <button class="mini-btn" title="还原窗口" @click="emit('toggle')">
        <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" fill="none">
          <path
            d="M1.2 4.4V1.2h3.2M10.8 4.4V1.2H7.6M1.2 7.6v3.2h3.2M10.8 7.6v3.2H7.6"
            stroke="currentColor"
            stroke-width="1.2"
            stroke-linecap="round"
          />
        </svg>
      </button>
      <button class="mini-btn" title="收进托盘（计时继续）" @click="emit('tray')">
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
</template>

<style scoped>
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

/* 动效偏好减弱：细线直接呈现 */
@media (prefers-reduced-motion: reduce) {
  .mini-progress {
    transition: none;
  }
}
</style>
