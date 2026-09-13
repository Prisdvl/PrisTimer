<script setup lang="ts">
// ---------------------------------------------------------------------------
// 番茄阶段条 + 设置面板：开启后常驻，循环推进全在 Rust 侧，这里只读状态。
// 点击展开设置面板（齿轮），循环参数即改即存；面板里还有可拖拽的
// 循环序列卡（拖「长休息」换位 = 改长休前的专注轮数）。
//
// P1-⑤ 第二步自 App.vue 拆出：状态全部来自 usePomodoro 模块级单例
// （与 App 的侧栏圆点/开关共享同一份 ref），无 props。
// ---------------------------------------------------------------------------

import { PHASE_LABEL, usePomodoro } from "../composables/usePomodoro";

const {
  pomodoro,
  pomoSettingsOpen,
  pomoFeedback,
  pomoDraft,
  applyPomoConfig,
  seqLocal,
  dragFrom,
  seqSnapping,
  onDropSeq,
  totalDots,
  litDots,
  SEQ_LABEL,
} = usePomodoro();
</script>

<template>
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
</template>

<style scoped>
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
  flex-wrap: wrap;
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

/* ---------------------------------------------------- 番茄序列（Draggable） */
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

/* 动效偏好减弱：进出场直接呈现 */
@media (prefers-reduced-motion: reduce) {
  .pomo-enter-active,
  .pomo-leave-active,
  .pomo-settings-enter-active,
  .pomo-settings-leave-active,
  .seq.snapping {
    transition: none;
    animation: none;
  }
}
</style>
