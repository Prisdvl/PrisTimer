<script setup lang="ts">
// ---------------------------------------------------------------------------
// 打字机提示：逐字输出一句话（光标闪烁），停顿一会后整条淡出。
//
// 用在「开始专注」的瞬间：顶端浮出「保持专注，慢慢来」这类提示，
// 打完字停留 ~1.4s 自己消失，不打断任何操作（pointer-events: none）。
// ---------------------------------------------------------------------------

import { onMounted, onUnmounted, ref } from "vue";

const props = withDefaults(
  defineProps<{
    text: string;
    /** 每个字符的输出间隔（ms）。 */
    speed?: number;
    /** 打完后的停留时间（ms），随后整条淡出并 emit done。 */
    holdMs?: number;
  }>(),
  { speed: 72, holdMs: 1400 },
);

const emit = defineEmits<{ done: [] }>();

const shown = ref("");
const fading = ref(false);
let timers: number[] = [];

onMounted(() => {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    shown.value = props.text;
    timers.push(window.setTimeout(() => emit("done"), props.holdMs));
    return;
  }
  [...props.text].forEach((_, i) => {
    timers.push(
      window.setTimeout(() => {
        shown.value = props.text.slice(0, i + 1);
      }, props.speed * (i + 1)),
    );
  });
  const doneAt = props.speed * props.text.length + props.holdMs;
  timers.push(
    window.setTimeout(() => {
      fading.value = true;
      timers.push(window.setTimeout(() => emit("done"), 360));
    }, doneAt),
  );
});

onUnmounted(() => timers.forEach(clearTimeout));
</script>

<template>
  <p class="tw" :class="{ bye: fading }" aria-live="polite">
    <span class="tw-text">{{ shown }}</span><span class="caret" />
  </p>
</template>

<style scoped>
.tw {
  display: flex;
  align-items: baseline;
  justify-content: center;
  gap: 2px;
  margin: 0;
  min-height: 1.5em;
  font-family: "Segoe UI", Consolas, monospace;
  font-size: 0.86rem;
  letter-spacing: 0.08em;
  color: var(--ink-soft);
  pointer-events: none;
  transition: opacity 0.35s ease, transform 0.35s ease;
}
.tw.bye {
  opacity: 0;
  transform: translateY(-6px);
}
.caret {
  width: 2px;
  height: 1em;
  background: var(--accent);
  align-self: center;
  animation: caret-blink 0.8s step-end infinite;
}
@keyframes caret-blink {
  50% {
    opacity: 0;
  }
}
@media (prefers-reduced-motion: reduce) {
  .caret {
    animation: none;
  }
  .tw {
    transition: none;
  }
}
</style>
