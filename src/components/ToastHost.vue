<script setup lang="ts">
// ---------------------------------------------------------------------------
// 提示宿主：把自己挂在 App 根部，订阅 useToast 的单例队列。
//
// 视觉延续磨砂玻璃语言：半透明面板 + 背景模糊 + 顶部高光边，
// 左侧一道状态色竖条表示语义，底部一条随 TTL 收缩的进度条表示"还有多久消失"。
// ---------------------------------------------------------------------------

import { dismissToast, toasts } from "../composables/useToast";
</script>

<template>
  <div class="toast-host" role="status" aria-live="polite">
    <TransitionGroup name="toast">
      <article
        v-for="item in toasts"
        :key="item.id"
        class="toast"
        :class="item.kind"
        @click="dismissToast(item.id)"
      >
        <span class="glyph">
          <svg v-if="item.kind === 'success'" viewBox="0 0 12 12" width="11" height="11">
            <path
              d="M2 6.4l2.6 2.6L10 3.4"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
          <svg v-else-if="item.kind === 'error'" viewBox="0 0 12 12" width="11" height="11">
            <path
              d="M6 2v5.2M6 9.6v.6"
              fill="none"
              stroke="currentColor"
              stroke-width="1.7"
              stroke-linecap="round"
            />
          </svg>
          <svg v-else viewBox="0 0 12 12" width="11" height="11">
            <path
              d="M6 5.4v3.2M6 2.6v.7"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
            />
          </svg>
        </span>

        <div class="body">
          <p class="title">{{ item.title }}</p>
          <p v-if="item.detail" class="detail">{{ item.detail }}</p>
        </div>
      </article>
    </TransitionGroup>
  </div>
</template>

<style scoped>
.toast-host {
  position: fixed;
  top: 50px;
  right: 14px;
  z-index: var(--z-toast);
  display: flex;
  flex-direction: column;
  gap: 0.45rem;
  align-items: flex-end;
  pointer-events: none;
}

/* 第十八轮简化：收掉左色条 / TTL 进度条 / 重投影 / hover 描边 ——
   语义色收敛到图标徽章一处，面板本体退成一块轻玻璃（更低模糊 = 更低
   合成开销），信息层级只剩"图标 + 标题 + 可选细节"。点击即消。 */
.toast {
  position: relative;
  display: flex;
  align-items: flex-start;
  gap: 0.5rem;
  width: min(21rem, 60vw);
  padding: 0.45rem 0.7rem 0.5rem 0.55rem;
  pointer-events: auto;
  cursor: pointer;
  background: var(--glass-bg-strong);
  border: 1px solid var(--glass-border);
  border-radius: 10px;
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-shadow);
}

.glyph {
  flex: none;
  display: grid;
  place-items: center;
  width: 18px;
  height: 18px;
  margin-top: 1px;
  border-radius: 50%;
  color: var(--tone);
  background: color-mix(in srgb, var(--tone) 15%, transparent);
}
.toast.success {
  --tone: #3ecf8e;
}
.toast.error {
  --tone: #ff8a8a;
}
.toast.info {
  --tone: #5aa7ff;
}

.body {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 0.1rem;
}
.title {
  margin: 0;
  font-size: 0.8rem;
  line-height: 1.4;
  color: var(--ink);
}
.detail {
  margin: 0;
  font-size: 0.68rem;
  line-height: 1.4;
  color: var(--ink-dim);
  /* 文件路径很长，允许在任意位置断开，否则会把面板撑破 */
  word-break: break-all;
}

/* ------------------------------------------------------------ 队列动画 */
/* 进场轻推入，退场淡出；move 让剩余条目平滑补位 */
.toast-enter-active {
  transition:
    opacity 0.24s var(--ease-out-expo),
    transform 0.24s var(--ease-out-expo);
}
.toast-leave-active {
  transition:
    opacity 0.18s ease-in,
    transform 0.18s ease-in;
  /* 退场时脱离文档流，其他条目才能平滑上移而不是瞬移 */
  position: absolute;
  right: 14px;
  width: min(21rem, 60vw);
}
.toast-move {
  transition: transform var(--t-base) var(--ease-out-expo);
}
.toast-enter-from {
  opacity: 0;
  transform: translateX(16px);
}
.toast-leave-to {
  opacity: 0;
  transform: translateX(12px);
}

@media (prefers-reduced-motion: reduce) {
  .toast-enter-active,
  .toast-leave-active,
  .toast-move {
    transition: none;
  }
}
</style>
