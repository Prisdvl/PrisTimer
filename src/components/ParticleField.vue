<script setup lang="ts">
// ---------------------------------------------------------------------------
// 粒子连线背景（参考集「Particle Text」的画布部分）。
//
// 一群缓慢漂移的微粒 + 距离小于阈值时互相连线，远看是"呼吸的星图"。
// 三个使用场景共用这一个组件：
//   · 启动动画的背景（IntroSplash）
//   · 冥想模式的慢速背景（speed 调低）
// 全程只做 canvas 2D 绘制，不碰 DOM 布局；页面隐藏时暂停 rAF，
// prefers-reduced-motion 时只画一帧静像。
// ---------------------------------------------------------------------------

import { onMounted, onUnmounted, ref } from "vue";

const props = withDefaults(
  defineProps<{
    /** 粒子数量。 */
    count?: number;
    /** 速度倍率：冥想模式传 <1 的值让整片慢下来。 */
    speed?: number;
    /** 连线距离（px）。 */
    link?: number;
    /** 粒子 rgb（逗号分隔三通道），默认柔和白。 */
    rgb?: string;
    /** 整体不透明度系数（0–1）。 */
    alpha?: number;
  }>(),
  { count: 46, speed: 1, link: 72, rgb: "255,255,255", alpha: 1 },
);

const canvas = ref<HTMLCanvasElement | null>(null);

interface P {
  x: number;
  y: number;
  vx: number;
  vy: number;
  r: number;
  a: number;
}

let raf = 0;
let pts: P[] = [];
let running = false;
let ctx: CanvasRenderingContext2D | null = null;
let ro: ResizeObserver | null = null;
let w = 0;
let h = 0;

/** 按容器实际尺寸 × DPR 建画布，返回逻辑宽高。 */
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
  // 尺寸变了以后粒子可能落在界外，按比例收回来
  for (const p of pts) {
    p.x = Math.min(p.x, w);
    p.y = Math.min(p.y, h);
  }
}

function seed(): void {
  pts = Array.from({ length: props.count }, () => ({
    x: Math.random() * w,
    y: Math.random() * h,
    vx: (Math.random() - 0.5) * 0.42 * props.speed,
    vy: (Math.random() - 0.5) * 0.42 * props.speed,
    r: Math.random() * 1.5 + 0.5,
    a: Math.random() * 0.42 + 0.16,
  }));
}

function frame(): void {
  if (!ctx) return;
  ctx.clearRect(0, 0, w, h);
  const link = props.link;
  for (const p of pts) {
    p.x += p.vx;
    p.y += p.vy;
    if (p.x < 0 || p.x > w) p.vx *= -1;
    if (p.y < 0 || p.y > h) p.vy *= -1;
    ctx.beginPath();
    ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
    ctx.fillStyle = `rgba(${props.rgb},${(p.a * props.alpha).toFixed(3)})`;
    ctx.fill();
  }
  for (let i = 0; i < pts.length; i += 1) {
    for (let j = i + 1; j < pts.length; j += 1) {
      const dx = pts[i].x - pts[j].x;
      const dy = pts[i].y - pts[j].y;
      const d = Math.hypot(dx, dy);
      if (d >= link) continue;
      ctx.beginPath();
      ctx.moveTo(pts[i].x, pts[i].y);
      ctx.lineTo(pts[j].x, pts[j].y);
      ctx.strokeStyle = `rgba(${props.rgb},${(0.1 * (1 - d / link) * props.alpha).toFixed(3)})`;
      ctx.lineWidth = 0.5;
      ctx.stroke();
    }
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
    frame();
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
  cancelAnimationFrame(raf);
  running = false;
  document.removeEventListener("visibilitychange", onVisibility);
  ro?.disconnect();
});
</script>

<template>
  <canvas ref="canvas" class="particles" aria-hidden="true" />
</template>

<style scoped>
.particles {
  position: absolute;
  inset: 0;
  pointer-events: none;
}
</style>
