<script setup lang="ts">
// ---------------------------------------------------------------------------
// 启动动画：Scramble 乱码解码出「PrisTimer」标题 + Particle 粒子背景。
//
// 目标总时长 ≈ 1.5s —— 节奏是硬编码的三段：
//   0–880ms  逐字解码（每 110ms 亮出一个真字符，其余位随机乱码翻滚）
//   880ms    开始淡出（0.3s）
//   ~1.21s   emit done，父组件卸载本层（淡出已走完大半，衔接无闪跳）
// reduced-motion 用户直接跳过（挂载即 emit done），不做任何动画。
// ---------------------------------------------------------------------------

import { onMounted, onUnmounted, ref } from "vue";
import ParticleField from "./ParticleField.vue";

const emit = defineEmits<{ done: [] }>();

const TITLE = "PrisTimer";
const POOL = "!<>-_\\/[]{}—=+*^?#";
const text = ref("");
const fading = ref(false);
let raf = 0;

onMounted(() => {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    emit("done");
    return;
  }
  const t0 = performance.now();
  const CHAR_MS = 110;
  const tick = (): void => {
    const shown = Math.floor((performance.now() - t0) / CHAR_MS);
    if (shown >= TITLE.length) {
      text.value = TITLE;
      // 收尾：短暂停留后淡出并交棒
      window.setTimeout(() => {
        fading.value = true;
        window.setTimeout(() => emit("done"), 330);
      }, 300);
      return;
    }
    let out = TITLE.slice(0, shown);
    for (let i = shown; i < TITLE.length; i += 1) {
      out += POOL[Math.floor(Math.random() * POOL.length)];
    }
    text.value = out;
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);
});

onUnmounted(() => cancelAnimationFrame(raf));
</script>

<template>
  <div class="splash" :class="{ bye: fading }">
    <ParticleField :count="58" :speed="1.5" :rgb="'132,120,255'" :alpha="0.8" />
    <span class="title">{{ text }}</span>
  </div>
</template>

<style scoped>
.splash {
  position: fixed;
  inset: 0;
  z-index: 200;
  display: grid;
  place-items: center;
  overflow: hidden;
  /* 与 .smoke 的底色同源：淡出时背景亮度无缝衔接，不会闪一下 */
  background: radial-gradient(120% 90% at 50% 0%, #12161d 0%, #0a0c10 55%, #07080b 100%);
  transition: opacity 0.3s ease;
}
.splash.bye {
  opacity: 0;
}
.title {
  position: relative;
  font-family: "Segoe UI Variable Display", "Segoe UI", Consolas, monospace;
  font-size: 2rem;
  font-weight: 600;
  letter-spacing: 0.14em;
  color: #a5aefc;
  text-shadow: 0 0 26px rgb(132 120 255 / 0.45);
  font-variant-numeric: tabular-nums;
  /* 乱码翻滚时宽度会抖，给个最小宽度让标题不左右跳 */
  min-width: 7.2em;
  text-align: center;
  white-space: nowrap;
}
</style>
