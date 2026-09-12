<script setup lang="ts">
// ---------------------------------------------------------------------------
// 方向性微粒场（第 21 轮，随暮霞/纸墨主题引入）。
//
// 与 ParticleField（漂移 + 距离连线，星图语系）的差异：
//   · 有宏观方向：dir="up" 微粒自下而上飘升（暮霞余烬火星，边升边熄）；
//     dir="down" 微粒缓缓沉降（纸墨浮尘落定）；
//   · 无连线：火星/浮尘是离散个体，连线会把"余烬"读成"星座"；
//   · 呼吸闪烁：每颗粒子自带 sin 相位的明暗脉动，像真的余烬在明灭。
//
// 全程 canvas 2D，只碰自身画布；页面隐藏时冻结 rAF；横向出界环绕，
// 纵向出界从另一端重播（上升的火星落到顶部即熄灭重生，永续循环）。
// ---------------------------------------------------------------------------

import { onMounted, onUnmounted, ref } from "vue";

const props = withDefaults(
  defineProps<{
    /** 微粒数量。 */
    count?: number;
    /** 宏观方向：up 上升（火星）/ down 沉降（浮尘）。 */
    dir?: "up" | "down";
    /** rgb 三元组字符串。 */
    rgb?: string;
    /** 上升/沉降基础速度（px/帧 @60fps）。 */
    speed?: number;
    /** 整体不透明度系数（0–1）。 */
    alpha?: number;
  }>(),
  { count: 22, dir: "up", rgb: "255,150,80", speed: 0.55, alpha: 1 },
);

const canvas = ref<HTMLCanvasElement | null>(null);

interface M {
  x: number;
  y: number;
  vy: number;
  vx: number;
  r: number;
  a: number;
  ph: number; // 闪烁相位
  tw: number; // 闪烁速率
}

let raf = 0;
let running = false;
let pts: M[] = [];
let ctx: CanvasRenderingContext2D | null = null;
let ro: ResizeObserver | null = null;
let w = 0;
let h = 0;

function resize(): void {
  const el = canvas.value;
  if (!el || !ctx) return;
  const box = el.parentElement?.getBoundingClientRect();
  w = Math.max(1, Math.round(box?.width ?? 0));
  h = Math.max(1, Math.round(box?.height ?? 0));
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  el.width = Math.round(w * dpr);
  el.height = Math.round(h * dpr);
  el.style.width = `${w}px`;
  el.style.height = `${h}px`;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}

function seed(): void {
  pts = Array.from({ length: props.count }, () => ({
    x: Math.random() * w,
    y: Math.random() * h,
    vy: (props.speed * (0.5 + Math.random() * 0.9)) * (props.dir === "up" ? -1 : 1),
    vx: (Math.random() - 0.5) * 0.12,
    r: Math.random() * 1.6 + 0.6,
    a: Math.random() * 0.45 + 0.25,
    ph: Math.random() * Math.PI * 2,
    tw: 0.6 + Math.random() * 1.6,
  }));
}

function frame(t: number): void {
  if (!ctx) return;
  const ts = t / 1000;
  ctx.clearRect(0, 0, w, h);
  for (const p of pts) {
    p.x += p.vx;
    p.y += p.vy;
    // 横向环绕；纵向出界从另一端重生（火星升顶即熄、浮尘落底即没）
    if (p.x < -4) p.x = w + 4;
    if (p.x > w + 4) p.x = -4;
    if (p.y < -6) { p.y = h + 6; p.x = Math.random() * w; }
    if (p.y > h + 6) { p.y = -6; p.x = Math.random() * w; }
    // sin 闪烁：亮度围绕自身 alpha 明灭
    const flick = 0.55 + 0.45 * Math.sin(ts * p.tw + p.ph);
    ctx.beginPath();
    ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
    ctx.fillStyle = `rgba(${props.rgb},${(p.a * flick * props.alpha).toFixed(3)})`;
    ctx.fill();
  }
  raf = requestAnimationFrame(frame);
}

function setRunning(on: boolean): void {
  if (on === running) return;
  running = on;
  cancelAnimationFrame(raf);
  if (on) raf = requestAnimationFrame(frame);
}

function onVisibility(): void {
  setRunning(!document.hidden);
}

onMounted(() => {
  const el = canvas.value;
  if (!el) return;
  ctx = el.getContext("2d");
  resize();
  seed();
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (reduced) {
    frame(0);
    cancelAnimationFrame(raf); // 只画一帧静像
    running = false;
  } else {
    setRunning(true);
    document.addEventListener("visibilitychange", onVisibility);
  }
  ro = new ResizeObserver(resize);
  if (el.parentElement) ro.observe(el.parentElement);
});

onUnmounted(() => {
  running = false;
  cancelAnimationFrame(raf);
  document.removeEventListener("visibilitychange", onVisibility);
  ro?.disconnect();
});
</script>

<template>
  <canvas ref="canvas" class="motes" aria-hidden="true" />
</template>

<style scoped>
.motes {
  position: absolute;
  inset: 0;
  pointer-events: none;
}
</style>
