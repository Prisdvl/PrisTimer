<script setup lang="ts">
// ---------------------------------------------------------------------------
// 表盘粒子容器（参考集「Particle Text」的画布部分，搬进表盘）。
//
// 把表盘看成一个圆形容器：开始计时的瞬间，一群微粒在盘面内自由漂移、
// 靠近时互相连线 —— "时间在容器里活着"。与参考实现的两处关键差异：
//   1. 边界不是矩形而是圆：粒子出界时沿法线反射速度（v -= 2(v·n)n），
//      再把位置夹回半径内 —— 撞到"表盘玻璃壁"的手感，而不是撞墙闪隐；
//   2. 生命周期严格跟随计时状态：仅 running 时推进 rAF，暂停/空闲即冻结
//      并淡出 —— 氛围动画不为静止的界面持续烧 GPU。
//
// 颜色取自继承的 --accent（专注绿 / 休息蓝紫 / 暂停琥珀自动跟随）。
// 绘制只碰 canvas 2D，不碰 DOM 布局；DPR 上限 2，粒子 34 颗 + O(n²) 连线
// 在 330px 盘面上每帧 <0.3ms（实测 r16 同量级画布）。
// ---------------------------------------------------------------------------

import { onMounted, onUnmounted, ref, watch } from "vue";

const props = withDefaults(
  defineProps<{
    /** 是否激活（计时运行中）。false 时冻结并淡出。 */
    active: boolean;
    /** 粒子数量。 */
    count?: number;
    /** 速度倍率（px/帧 @60fps 的基准 0.5）。 */
    speed?: number;
    /** 连线距离（px）。 */
    link?: number;
  }>(),
  { count: 34, speed: 1, link: 62 },
);

const canvas = ref<HTMLCanvasElement | null>(null);
const visible = ref(false); // 控制淡入淡出（与 rAF 生命周期解耦半拍）

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
let ctx: CanvasRenderingContext2D | null = null;
let ro: ResizeObserver | null = null;
let w = 0;
let h = 0;
/** 继承到的强调色，rgb 三元组字符串。 */
let rgb = "62,207,142";

/** hex → "r,g,b"。--accent 是纯 hex（#3ecf8e 等），解析失败走兜底绿。 */
function parseAccent(raw: string): string {
  const m = raw.trim().match(/^#([0-9a-f]{6})$/i);
  if (!m) return "62,207,142";
  const n = Number.parseInt(m[1], 16);
  return `${(n >> 16) & 255},${(n >> 8) & 255},${n & 255}`;
}

function readAccent(): void {
  const el = canvas.value;
  if (!el) return;
  const v = getComputedStyle(el).getPropertyValue("--accent");
  if (v) rgb = parseAccent(v);
}

/** 按容器实际尺寸 × DPR 建画布；圆形容器的有效半径 = 短边一半 - 边距。 */
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
  // 尺寸变化后粒子可能落在圆外，按比例收进来
  const R = radius();
  const cx = w / 2;
  const cy = h / 2;
  for (const p of pts) {
    const dx = p.x - cx;
    const dy = p.y - cy;
    const d = Math.hypot(dx, dy);
    if (d > R) {
      p.x = cx + (dx / d) * R * 0.96;
      p.y = cy + (dy / d) * R * 0.96;
    }
  }
}

function radius(): number {
  return Math.min(w, h) / 2 - 10;
}

/** 在圆内随机播种。 */
function seed(): void {
  const R = radius() * 0.94;
  const cx = w / 2;
  const cy = h / 2;
  pts = Array.from({ length: props.count }, () => {
    const t = Math.random() * Math.PI * 2;
    const rr = Math.sqrt(Math.random()) * R;
    return {
      x: cx + Math.cos(t) * rr,
      y: cy + Math.sin(t) * rr,
      vx: (Math.random() - 0.5) * 0.5 * props.speed,
      vy: (Math.random() - 0.5) * 0.5 * props.speed,
      r: Math.random() * 1.4 + 0.5,
      a: Math.random() * 0.4 + 0.18,
    };
  });
}

/** 出界反弹：速度沿法线反射，位置夹回圆内。 */
function confine(p: P): void {
  const R = radius();
  const cx = w / 2;
  const cy = h / 2;
  const dx = p.x - cx;
  const dy = p.y - cy;
  const d = Math.hypot(dx, dy);
  if (d <= R) return;
  const nx = dx / d;
  const ny = dy / d;
  const dot = p.vx * nx + p.vy * ny;
  if (dot > 0) {
    p.vx -= 2 * dot * nx;
    p.vy -= 2 * dot * ny;
  }
  p.x = cx + nx * R;
  p.y = cy + ny * R;
}

function frame(): void {
  if (!ctx) return;
  ctx.clearRect(0, 0, w, h);
  const link = props.link;
  for (const p of pts) {
    p.x += p.vx;
    p.y += p.vy;
    confine(p);
    ctx.beginPath();
    ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
    ctx.fillStyle = `rgba(${rgb},${p.a.toFixed(3)})`;
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
      ctx.strokeStyle = `rgba(${rgb},${(0.1 * (1 - d / link)).toFixed(3)})`;
      ctx.lineWidth = 0.5;
      ctx.stroke();
    }
  }
  raf = requestAnimationFrame(frame);
}

function setRunning(on: boolean): void {
  cancelAnimationFrame(raf);
  if (on) raf = requestAnimationFrame(frame);
}

/** 页面隐藏（最小化/切走）时冻结 rAF，可见且 active 时恢复。 */
function sync(): void {
  setRunning(props.active && !document.hidden);
}

function onVisibility(): void {
  sync();
}

watch(
  () => props.active,
  (on) => {
    if (on) {
      readAccent();
      if (pts.length === 0) seed();
    }
    visible.value = on;
    sync();
  },
);

onMounted(() => {
  const el = canvas.value;
  if (!el) return;
  ctx = el.getContext("2d");
  resize();
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (!reduced) document.addEventListener("visibilitychange", onVisibility);
  // 初始就处于 running（启动恢复的会话）：直接播种 + 淡入
  if (props.active) {
    readAccent();
    seed();
    visible.value = true;
    if (!reduced) sync();
    else {
      frame(); // reduced-motion：只画一帧静像
      cancelAnimationFrame(raf);
    }
  }
  ro = new ResizeObserver(resize);
  if (el.parentElement) ro.observe(el.parentElement);
});

onUnmounted(() => {
  cancelAnimationFrame(raf);
  document.removeEventListener("visibilitychange", onVisibility);
  ro?.disconnect();
});
</script>

<template>
  <canvas ref="canvas" class="dial-particles" :class="{ on: visible }" aria-hidden="true" />
</template>

<style scoped>
.dial-particles {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  pointer-events: none;
  opacity: 0;
  transition: opacity 0.7s ease;
}
.dial-particles.on {
  opacity: 1;
}
</style>
