<script setup lang="ts">
// ---------------------------------------------------------------------------
// 指针式表盘。
//
// 为什么是"秒表"式映射而不是挂钟式：盘面读的是**一段时长**，不是时刻。
// 所以长针一圈 = 60 秒、短针一圈 = 60 分钟，双针同时转 —— 这正是一块
// 机械秒表的心智模型，比把 25 分钟硬塞进 12 小时刻度直观得多。
//
// 丝滑的两个关键：
//   1. 引擎每 100ms 推一帧（TICK_MS），角度变化足够密，配 0.13s 线性
//      transition 就能把离散更新连成连续转动；
//   2. 角度**单调累加**，不做 %360 直接赋值 —— 否则 59 秒跳到 0 秒时
//      指针会倒着甩一整圈。归零/重置这种真·回退则关掉过渡直接归位。
// ---------------------------------------------------------------------------

import { computed, ref, watch } from "vue";
import DialParticles from "./DialParticles.vue";

/** 主题标识：透传给表盘粒子取色（App 的主题色板驱动）。 */
export type DialTheme = "deep" | "void" | "dawn" | "aurora" | "ember" | "paper";

const props = defineProps<{
  /** 已进行的毫秒数（倒计时取 limit - remaining）。 */
  elapsedMs: number;
  /** 倒计时进度 0–1；正计时传 null（无目标，不画弧）。 */
  progress: number | null;
  state: "idle" | "running" | "paused" | "finished";
  /** 当前背景主题（第 20 轮起指针/粒子随主题换色）。 */
  theme?: DialTheme;
  /** 第 22 轮简约模式：不渲染盘内粒子层。 */
  simple?: boolean;
}>();

/** 表盘粒子容器：计时运行时盘内粒子飘动（时间即文字，盘即容器）。
 *  简约模式下整层不挂载（组件级关闭，canvas 不创建、零开销）。 */
const particlesOn = computed(() => props.state === "running" && !props.simple);

const C = 160; // 盘心
const R_TICK_OUT = 146;
const R_TICK_MIN = 138;
const R_TICK_MAJOR = 130;

/** 60 根刻度；每 5 根加长加亮，作为"分钟刻度组"的视觉锚点。 */
const ticks = Array.from({ length: 60 }, (_, i) => {
  const major = i % 5 === 0;
  const angle = (i * 6 * Math.PI) / 180;
  const inner = major ? R_TICK_MAJOR : R_TICK_MIN;
  return {
    i,
    major,
    x1: C + Math.sin(angle) * inner,
    y1: C - Math.cos(angle) * inner,
    x2: C + Math.sin(angle) * R_TICK_OUT,
    y2: C - Math.cos(angle) * R_TICK_OUT,
  };
});

const R_ARC = 150;
const ARC_CIRC = 2 * Math.PI * R_ARC;
const arcDash = computed(() => {
  const p = props.progress ?? 0;
  return `${ARC_CIRC * p} ${ARC_CIRC}`;
});

// ---- 双针角度：单调累加，永不倒甩 ----------------------------------------

const seconds = computed(() => Math.floor(props.elapsedMs / 1000));
const minutes = computed(() => Math.floor(props.elapsedMs / 60_000));

const longAngle = ref((seconds.value % 60) * 6);
const shortAngle = ref((minutes.value % 60) * 6);
/** 归零/重置这类真回退：关一帧过渡，指针直接归位而不是倒着狂转。 */
const snap = ref(false);

function snapHands(): void {
  snap.value = true;
  longAngle.value = (seconds.value % 60) * 6;
  shortAngle.value = (minutes.value % 60) * 6;
  requestAnimationFrame(() => {
    snap.value = false;
  });
}

watch(seconds, (now, prev) => {
  if (prev === undefined) return;
  const step = now - prev;
  if (step < 0) return snapHands();
  longAngle.value += step * 6;
});

watch(minutes, (now, prev) => {
  if (prev === undefined) return;
  const step = now - prev;
  if (step < 0) return; // 秒针的 watch 已经统一处理过回退
  shortAngle.value += step * 6;
});
</script>

<template>
  <div class="dial" :class="[state, { snap }]">
    <svg class="face" viewBox="0 0 320 320" aria-hidden="true">
      <!-- 倒计时进度弧：细、贴边，只承担"还剩多少"这一件事 -->
      <g v-if="progress !== null" class="arc-group">
        <circle class="arc-track" :cx="C" :cy="C" :r="R_ARC" />
        <circle class="arc" :cx="C" :cy="C" :r="R_ARC" :stroke-dasharray="arcDash" />
      </g>

      <!-- 刻度圈 -->
      <line
        v-for="t in ticks"
        :key="t.i"
        class="tick"
        :class="{ major: t.major }"
        :x1="t.x1"
        :y1="t.y1"
        :x2="t.x2"
        :y2="t.y2"
      />

      <!-- 短针（分） -->
      <g class="hand hour" :style="{ transform: `rotate(${shortAngle}deg)` }">
        <rect class="hand-body" x="155" y="98" width="10" height="70" rx="5" />
      </g>
      <!-- 长针（秒） -->
      <g class="hand minute" :style="{ transform: `rotate(${longAngle}deg)` }">
        <rect class="hand-body" x="157.5" y="56" width="5" height="110" rx="2.5" />
      </g>

      <!-- 轴帽 -->
      <circle class="hub-ring" :cx="C" :cy="C" r="7" />
      <circle class="hub" :cx="C" :cy="C" r="3.6" />
    </svg>

    <!-- 粒子容器：运行时粒子在盘内漂移连线（在刻度之上、盘心读数之下，
         不与指针/文字抢焦点；第 20 轮起取色随主题而非状态 —— 状态色
         交给边框/弧/呼吸光承担，粒子负责"这个主题长这样"。
         第 22 轮：简约模式下整层不挂载） -->
    <DialParticles v-if="!simple" :active="particlesOn" :theme="theme" />

    <!-- 盘心读数由父组件注入（它负责数字补间动画） -->
    <div class="core">
      <slot />
    </div>

    <!-- 运行时的呼吸光：让静止的玻璃盘"活着"，也提示指针正在走 -->
    <span class="breath" aria-hidden="true" />
  </div>
</template>

<style scoped>
.dial {
  position: relative;
  /* 尺寸由父级 --dial-size 决定（随窗口高度伸缩），缺省 320px */
  width: var(--dial-size, 320px);
  height: var(--dial-size, 320px);
  border-radius: 50%;
  /* 玻璃圆盘：无网格。只有一层极淡的斜向高光模拟玻璃厚度，
     真正的"质感"来自 backdrop-filter 模糊背后的烟雾。 */
  background:
    linear-gradient(158deg, rgb(255 255 255 / 0.055), transparent 42%),
    radial-gradient(circle at 50% 118%, rgb(255 255 255 / 0.035), transparent 60%),
    rgb(255 255 255 / 0.022);
  border: 1px solid var(--glass-border);
  backdrop-filter: var(--glass-blur-lg);
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.09),
    inset 0 -20px 40px rgb(0 0 0 / 0.18),
    0 24px 60px rgb(0 0 0 / 0.42);
  transition: border-color 0.55s ease, box-shadow 0.55s ease;
}
.running.dial {
  border-color: color-mix(in srgb, var(--accent) 38%, transparent);
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.09),
    inset 0 -20px 40px rgb(0 0 0 / 0.18),
    0 24px 60px rgb(0 0 0 / 0.42),
    0 0 52px color-mix(in srgb, var(--accent) 16%, transparent);
}

.face {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
}

/* ------------------------------------------------------------------ 刻度 */
.tick {
  /* 第 20 轮：白色固定值换主题令牌 —— 晨雾浅底上白刻度原本不可见 */
  stroke: var(--tick);
  stroke-width: 1.4;
  stroke-linecap: round;
  transition: stroke 0.5s ease;
}
.tick.major {
  stroke: var(--tick-strong);
  stroke-width: 2.2;
}
.running .tick.major {
  stroke: color-mix(in srgb, var(--accent) 55%, rgb(255 255 255 / 0.2));
}

/* ------------------------------------------------------------------ 指针 */
.hand {
  /* transform-box: view-box 让 160px 160px 落在盘心，
     而不是每根针自己的包围盒中心（fill-box 会绕着针身转，错得离谱） */
  transform-box: view-box;
  transform-origin: 160px 160px;
  transition: transform 0.13s linear;
}
.snap .hand {
  transition: none;
}
.hand-body {
  /* 第 20 轮：四态指针色全部主题化（--hand-* 令牌，随主题丝滑插值） */
  fill: var(--hand-rest);
  transition: fill 0.45s ease, filter 0.45s ease;
}
.running .hand-body {
  /* 第 22 轮：运行指针也主题化 —— 默认仍是状态绿（--accent 兜底），
     浅色主题（dawn/paper）在 glass.css 覆盖 --hand-running 为深色：
     绿色在浅玻璃底上对比只有 1.7:1，老大实测"启动后看不清"。 */
  fill: var(--hand-running, var(--accent));
  filter: drop-shadow(0 0 6px color-mix(in srgb, var(--hand-running, var(--accent)) 55%, transparent));
}
.paused .hand-body {
  fill: var(--hand-paused);
}
.finished .hand-body {
  fill: var(--hand-finished);
}
/* 停住时指针略微变细变暗，视觉上"松开了" */
.idle .hand-body {
  fill: var(--hand-idle);
}

/* 小时针比秒针短粗，两者色相相同、明度上略作区分 */
.hand.hour .hand-body {
  opacity: 0.92;
}

.hub-ring {
  fill: rgb(10 12 16 / 0.85);
  stroke: rgb(255 255 255 / 0.12);
  stroke-width: 1.2;
}
.hub {
  fill: var(--accent);
  transition: fill 0.45s ease;
  filter: drop-shadow(0 0 7px color-mix(in srgb, var(--accent) 70%, transparent));
}

/* ------------------------------------------------------------------ 弧 */
.arc-group {
  transform-box: view-box;
  transform-origin: 160px 160px;
  transform: rotate(-90deg);
}
.arc-track,
.arc {
  fill: none;
  stroke-width: 3;
  stroke-linecap: round;
}
.arc-track {
  stroke: color-mix(in srgb, var(--tick-strong) 28%, transparent);
}
.arc {
  stroke: var(--accent);
  transition: stroke-dasharray 0.4s var(--ease-out-quart), stroke 0.45s ease;
  filter: drop-shadow(0 0 7px color-mix(in srgb, var(--accent) 60%, transparent));
}

/* ------------------------------------------------------------------ 盘心 */
.core {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.breath {
  position: absolute;
  inset: -28px;
  border-radius: 50%;
  pointer-events: none;
  background: radial-gradient(circle at 50% 50%, color-mix(in srgb, var(--accent) 16%, transparent), transparent 66%);
  opacity: 0;
  transition: opacity 0.6s ease;
}
.running .breath {
  opacity: 1;
  animation: breathe 3.6s var(--ease-in-out-soft) infinite;
}
@keyframes breathe {
  50% {
    transform: scale(1.05);
    opacity: 0.55;
  }
}

@media (prefers-reduced-motion: reduce) {
  .hand {
    transition: none;
  }
  .breath {
    animation: none;
  }
}
</style>
