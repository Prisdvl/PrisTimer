<script setup lang="ts">
// ---------------------------------------------------------------------------
// 动态背景：八团弥散的"烟"（常驻基底） + 按主题切换的特色动效层。
//
//   · 基底（全主题）：烟团慢漂移 + 明暗呼吸 + 颗粒噪点 —— 氛围的底味；
//   · 深空 deep：三条斜向光带扫过（流动的暗示）；
//   · 虚空 void：烟几乎退场，换 90 颗闪烁星尘 + 偶发流星（GSAP 划过）；
//   · 极光 aurora：三幅纵向光幕缓摆（极夜天穹）；
//   · 晨雾 dawn：暖色光尘缓飘（日间的浮埃）。
//
// ★ 主题切换的观感：颜色插值交给 glass.css 的 @property + :root transition
//   （底色/烟色/墨色整体渐变 0.9s）；这里的特色层只做交叉淡入淡出，
//   两者叠加出"世界平滑换景"的效果。切换时仅重建特色层补间，
//   烟团补间不动（避免运动相位被重置）。
//
// 全部补间只碰 transform / opacity（GPU 合成属性），不做 filter: blur()。
// ---------------------------------------------------------------------------

import { nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { gsap } from "gsap";
import ParticleField from "./ParticleField.vue";

export type BgTheme = "deep" | "void" | "dawn" | "aurora";

const props = withDefaults(
  defineProps<{
    /** 迷你模式下冻结全部时间线（小窗背后纯耗电）。 */
    paused?: boolean;
    /** 当前主题（App 的主题色板驱动）。 */
    theme?: BgTheme;
    /** 计时进行中：背景整体提速（世界跟着专注呼吸加快）。 */
    active?: boolean;
  }>(),
  { paused: false, theme: "deep", active: false },
);

const root = ref<HTMLElement | null>(null);
const meteorLayer = ref<HTMLElement | null>(null);

/** 基底补间（烟团）：主题切换不重建，运动相位不丢。 */
const baseTweens: gsap.core.Tween[] = [];
/** 主题特色补间（光带/光幕/流星调度）：随主题重建。 */
const fxTweens: gsap.core.Tween[] = [];
let meteorCall: ReturnType<typeof gsap.delayedCall> | null = null;

const reduced = (): boolean =>
  window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** 统一暂停/恢复（幂等）。 */
function setPaused(on: boolean): void {
  for (const t of baseTweens) (on ? t.pause() : t.resume());
  for (const t of fxTweens) (on ? t.pause() : t.resume());
  if (meteorCall) (on ? meteorCall.pause() : meteorCall.resume());
}

/** 状态联动：计时运行时整体提速 1.5x（idle/暂停回落 1x）。只改速率不改相位。 */
function setActivity(active: boolean): void {
  const s = active ? 1.5 : 1;
  for (const t of baseTweens) t.timeScale(s);
  for (const t of fxTweens) t.timeScale(s);
}

/** 页面隐藏（最小化/切走）时冻结；恢复可见且未被外部门控时继续。 */
function onVisibility(): void {
  setPaused(document.hidden || !!props.paused);
}

// ---- 基底：烟团 -----------------------------------------------------------

function sec(el: HTMLElement, name: string, fallback: number): number {
  const v = Number.parseFloat(getComputedStyle(el).getPropertyValue(name));
  return Number.isFinite(v) && v > 0 ? v : fallback;
}

function animatePuff(el: HTMLElement): void {
  // p7/p8 是反向烟缕（与大盘反向漂移，避免整张背景同向平移的错觉）
  const reverse = el.classList.contains("puff-7") || el.classList.contains("puff-8");
  const dur = sec(el, "--dur", reverse ? 16 : 60);
  const bdur = sec(el, "--bdur", 17);
  const delay = Number.parseFloat(getComputedStyle(el).getPropertyValue("--delay")) || 0;

  const drift = reverse
    ? gsap.fromTo(
        el,
        { xPercent: 0, yPercent: 0, scale: 1.18 },
        { xPercent: -19, yPercent: -14, scale: 0.86, duration: dur, ease: "sine.inOut" },
      )
    : gsap.fromTo(
        el,
        { xPercent: 0, yPercent: 0, scale: 1 },
        { xPercent: 16, yPercent: 12, scale: 1.34, duration: dur, ease: "sine.inOut" },
      );
  drift.repeat(-1).yoyo(true);
  drift.progress((Math.abs(delay) / dur) % 1);
  baseTweens.push(drift);

  const omin = Number.parseFloat(getComputedStyle(el).getPropertyValue("--omin")) || 0.62;
  const omax = Number.parseFloat(getComputedStyle(el).getPropertyValue("--omax")) || 1;
  const breathe = gsap.fromTo(
    el,
    { opacity: omin },
    { opacity: omax, duration: bdur, ease: "sine.inOut" },
  );
  breathe.repeat(-1).yoyo(true);
  breathe.progress(((Math.abs(delay) / 2) / bdur) % 1);
  baseTweens.push(breathe);
}

// ---- 主题特色层 -----------------------------------------------------------

/** 虚空星尘：位置/尺寸/闪烁周期全部随机，CSS 动画自转（不占 GSAP ticker）。 */
const stars = ref<Array<{ id: number; style: Record<string, string> }>>([]);

function genStars(): void {
  stars.value = Array.from({ length: 90 }, (_, i) => ({
    id: i,
    style: {
      left: `${(Math.random() * 100).toFixed(2)}%`,
      top: `${(Math.random() * 100).toFixed(2)}%`,
      width: `${(0.8 + Math.random() * 1.8).toFixed(2)}px`,
      height: `${(0.8 + Math.random() * 1.8).toFixed(2)}px`,
      animationDuration: `${(1.6 + Math.random() * 3.6).toFixed(2)}s`,
      animationDelay: `-${(Math.random() * 4).toFixed(2)}s`,
    },
  }));
}

/** 虚空流星：随机起点划过左下，亮尾渐隐；2.5–6s 一颗，15% 概率双发。 */
function spawnMeteor(): void {
  const layer = meteorLayer.value;
  if (!layer) return;
  const launch = (delay = 0): void => {
    const el = document.createElement("span");
    el.className = "meteor";
    el.style.left = `${(18 + Math.random() * 72).toFixed(1)}%`;
    el.style.top = `${(4 + Math.random() * 34).toFixed(1)}%`;
    layer.appendChild(el);
    gsap.fromTo(
      el,
      { x: 0, y: 0, opacity: 0, rotation: 148 },
      {
        x: -240 - Math.random() * 120,
        y: 150 + Math.random() * 80,
        opacity: 1,
        duration: 0.55,
        delay,
        ease: "power2.out",
        onComplete: () => {
          gsap.to(el, { opacity: 0, duration: 0.3, onComplete: () => el.remove() });
        },
      },
    );
  };
  launch();
  if (Math.random() < 0.15) launch(0.22);
}

function scheduleMeteor(): void {
  meteorCall?.kill();
  meteorCall = gsap.delayedCall(2.5 + Math.random() * 3.5, () => {
    spawnMeteor();
    scheduleMeteor();
  });
  fxTweens.push(meteorCall as unknown as gsap.core.Tween);
}

/** 主题特色补间：随主题卸载重建（烟团不在其列）。 */
async function buildThemeFx(): Promise<void> {
  // 清旧：特色补间全杀 + 流星 DOM 清场
  for (const t of fxTweens) t.kill();
  fxTweens.length = 0;
  meteorCall = null;
  await nextTick();
  const el = root.value;
  if (!el) return;

  // 深空光带：斜向扫过 + 明暗呼吸（沿用 glass.css 的 --sdur/--sbdur）
  el.querySelectorAll<HTMLElement>(".beam").forEach((beam) => {
    const sdur = sec(beam, "--sdur", 30);
    const sbdur = sec(beam, "--sbdur", 15);
    const sdelay = Number.parseFloat(getComputedStyle(beam).getPropertyValue("--sdelay")) || 0;
    const sweep = gsap.fromTo(
      beam,
      { xPercent: -26, rotation: -17, scaleY: 1 },
      { xPercent: 30, rotation: -9, scaleY: 1.14, duration: sdur, ease: "sine.inOut" },
    );
    sweep.repeat(-1).yoyo(true);
    sweep.progress((Math.abs(sdelay) / sdur) % 1);
    fxTweens.push(sweep);

    const breathe = gsap.fromTo(
      beam,
      { opacity: 0.62 },
      { opacity: 1, duration: sbdur, ease: "sine.inOut" },
    );
    breathe.repeat(-1).yoyo(true);
    breathe.progress(((Math.abs(sdelay) / 3) / sbdur) % 1);
    fxTweens.push(breathe);
  });

  // 极光光幕：纵向缓摆 + 明暗呼吸（每幅周期错开，避免齐摆的机械感）
  el.querySelectorAll<HTMLElement>(".curtain").forEach((c, i) => {
    const dur = sec(c, "--cdur", 18 + i * 5);
    const sway = gsap.fromTo(
      c,
      { yPercent: -7, opacity: 0.45 },
      { yPercent: 9, opacity: 1, duration: dur, ease: "sine.inOut" },
    );
    sway.repeat(-1).yoyo(true);
    sway.progress(Math.random());
    fxTweens.push(sway);
  });

  // 虚空流星：reduced-motion 不调度（星尘的 CSS 闪烁也由媒体查询关掉）
  if (props.theme === "void" && !reduced()) scheduleMeteor();

  // 重建后保持当前活跃度（计时进行中 = 1.5x）
  setActivity(props.active);
  if (props.paused || document.hidden) setPaused(true);
}

watch(
  () => [props.theme, props.paused] as const,
  () => {
    if (props.theme === "void") genStars();
    void buildThemeFx();
  },
);

// 状态联动：开始/暂停计时 → 背景呼吸整体加速/回落（速率切换，相位不跳）
watch(
  () => props.active,
  (on) => {
    if (!reduced()) setActivity(on);
  },
);

onMounted(() => {
  const el = root.value;
  if (!el) return;
  if (props.theme === "void") genStars();

  // reduced-motion：一套补间都不建，元素停在 from 帧静态呈现
  if (!reduced()) {
    el.querySelectorAll<HTMLElement>(".puff").forEach(animatePuff);
    // 全屏明暗脉动：一层 --ink 派生的薄纱呼吸（深色=白雾，晨雾=云影）
    const pulse = el.querySelector<HTMLElement>(".fx-pulse");
    if (pulse) {
      const breathe = gsap.fromTo(
        pulse,
        { opacity: 0.25 },
        { opacity: 0.9, duration: 13, ease: "sine.inOut" },
      );
      breathe.repeat(-1).yoyo(true);
      baseTweens.push(breathe);
    }
    void buildThemeFx();
    document.addEventListener("visibilitychange", onVisibility);
    // 初始就是迷你形态（上次退出时收成了组件）：直接停在冻结态
    if (props.paused) setPaused(true);
    setActivity(props.active);
  }
});

onUnmounted(() => {
  for (const t of baseTweens) t.kill();
  for (const t of fxTweens) t.kill();
  baseTweens.length = 0;
  fxTweens.length = 0;
  meteorCall?.kill();
  document.removeEventListener("visibilitychange", onVisibility);
});
</script>

<template>
  <div ref="root" class="smoke" aria-hidden="true">
    <span v-for="n in 8" :key="`p${n}`" class="puff" :class="`puff-${n}`" />

    <!-- 深空：斜向光带 + 冷色光尘 -->
    <Transition name="layerfade">
      <div v-if="theme === 'deep'" class="fx">
        <span class="beam beam-1" />
        <span class="beam beam-2" />
        <span class="beam beam-3" />
        <ParticleField :count="18" :speed="0.22" rgb="150,170,255" :alpha="0.7" />
      </div>
    </Transition>

    <!-- 虚空：星尘 + 流星 -->
    <Transition name="layerfade">
      <div v-if="theme === 'void'" class="fx">
        <span
          v-for="s in stars"
          :key="s.id"
          class="star"
          :style="s.style"
        />
        <div ref="meteorLayer" class="meteor-layer" />
      </div>
    </Transition>

    <!-- 极光：纵向光幕 + 绿青光尘 -->
    <Transition name="layerfade">
      <div v-if="theme === 'aurora'" class="fx">
        <span class="curtain c1" />
        <span class="curtain c2" />
        <span class="curtain c3" />
        <ParticleField :count="16" :speed="0.26" rgb="120,255,200" :alpha="0.6" />
      </div>
    </Transition>

    <!-- 晨雾：暖色光尘 -->
    <Transition name="layerfade">
      <div v-if="theme === 'dawn'" class="fx">
        <ParticleField :count="26" :speed="0.3" rgb="255,226,180" :alpha="0.85" />
      </div>
    </Transition>

    <!-- 全屏明暗脉动：--ink 派生薄纱（深色主题=白雾律动，晨雾=云影掠过） -->
    <span class="fx-pulse" />

    <span class="grain" />
  </div>
</template>

<style scoped>
/* 特色层容器：铺满 .smoke，切换时整层交叉淡入淡出 */
.fx {
  position: absolute;
  inset: 0;
  overflow: hidden;
}
.layerfade-enter-active,
.layerfade-leave-active {
  transition: opacity 0.9s var(--ease-in-out-soft);
}
.layerfade-enter-from,
.layerfade-leave-to {
  opacity: 0;
}

/* 全屏明暗脉动层：颜色从 --ink 派生 —— 深色主题下是顶部白雾缓缓明暗，
   晨雾下 ink 是暗色，同一份代码自动变成"云影掠过"。opacity 由 GSAP 驱动。 */
.fx-pulse {
  position: absolute;
  inset: 0;
  background: radial-gradient(
    62% 46% at 50% 6%,
    color-mix(in srgb, var(--ink) 8%, transparent),
    transparent 72%
  );
  opacity: 0.25;
  will-change: opacity;
}

/* 虚空星尘 */
.star {
  position: absolute;
  border-radius: 50%;
  background: rgb(255 255 255 / 0.85);
  box-shadow: 0 0 6px rgb(255 255 255 / 0.35);
  animation: star-twinkle ease-in-out infinite;
}
@keyframes star-twinkle {
  0%,
  100% {
    opacity: 0.12;
    transform: scale(0.8);
  }
  50% {
    opacity: 0.9;
    transform: scale(1.15);
  }
}

/* 虚空流星：亮头拖尾的一条细线，基础方向由 GSAP rotation 给出 */
.meteor-layer {
  position: absolute;
  inset: 0;
}
.meteor {
  position: absolute;
  width: 130px;
  height: 1.5px;
  border-radius: 999px;
  background: linear-gradient(90deg, rgb(255 255 255 / 0.95), rgb(255 255 255 / 0));
}

/* 极光光幕：竖向渐变带；左右边缘用 mask 软化（静态遮罩，零每帧成本），
   不用 filter（大元素红线） */
.curtain {
  position: absolute;
  top: -18%;
  bottom: -18%;
  width: 20rem;
  background: linear-gradient(
    180deg,
    transparent,
    color-mix(in srgb, var(--smoke-a) 30%, transparent) 30%,
    color-mix(in srgb, var(--smoke-b) 22%, transparent) 62%,
    transparent 92%
  );
  transform: skewX(-7deg);
  -webkit-mask-image: linear-gradient(90deg, transparent, #000 22%, #000 78%, transparent);
  mask-image: linear-gradient(90deg, transparent, #000 22%, #000 78%, transparent);
  will-change: transform, opacity;
}
.curtain.c1 {
  --cdur: 17s;
  left: 6%;
}
.curtain.c2 {
  --cdur: 23s;
  left: 38%;
  width: 26rem;
}
.curtain.c3 {
  --cdur: 29s;
  left: 68%;
}

@media (prefers-reduced-motion: reduce) {
  .star {
    animation: none;
    opacity: 0.55;
  }
  .layerfade-enter-active,
  .layerfade-leave-active {
    transition: none;
  }
}
</style>
