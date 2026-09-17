<script setup lang="ts">
// ---------------------------------------------------------------------------
// 悬浮信息窗：贴托盘旁的小窗口，**主窗口最小化后仍持续展示**环境信息。
//
// 由 `insight::commands::overlay_toggle` 控制显隐（Rust 侧负责定位到托盘旁），
// 本组件只负责在给定的窗口里把信息渲染出来。
//
// ★ 版式（2026-09-17 重排，用户要求"主体是额度，耳机电量用图标、尽量小"）：
//
//   ┌──────────────────────────────┐
//   │ 本月额度            9/17 重置 │  ← 标题行
//   │ 87%                          │  ← 主体：大号剩余百分比
//   │ ▓▓▓▓▓▓▓▓▓░░░                 │  ← 进度条（宽度带过渡动画）
//   │ 滚动 100% · 本周 74%          │  ← 其余窗口压成一行小字
//   │ 🎧 MT6 70%  ⌨ SC580SE 57%    │  ← 蓝牙设备：图标 + 短名 + 电量
//   └──────────────────────────────┘
//
//   玻璃与圆角沿用主界面「开始」按钮那一套变量（--glass-bg / --glass-border
//   / --glass-blur），两个界面的质感才是同一套语言，而不是各写各的。
//
// 与主窗口状态栏共用 `useInsight`：各窗口的 JS 上下文各自订阅
// `insight:update`（Tauri 事件广播到所有窗口），数据永远同步。
// ---------------------------------------------------------------------------

import { computed } from "vue";
import {
  useInsight,
  batteryText,
  shortName,
  deviceLine,
  quotaLine,
  ageLabel,
} from "../composables/useInsight";

const { snapshot, quotaStale } = useInsight();

/** 多窗口额度（opencode Go：滚动 / 本周 / 本月）。空数组 = 单值接口。 */
const quotaWindows = computed(() =>
  snapshot.value.quota.status === "ok" ? snapshot.value.quota.windows ?? [] : [],
);

/**
 * 主体只放「本月」这一档 —— 用户明确要求额度区以本月为主。
 * 找不到"本月"（非 opencode Go 接口）就退回最后一档，再不然走单值文案。
 */
const monthWindow = computed(() => {
  const list = quotaWindows.value;
  if (!list.length) return null;
  return list.find((w) => w.label.includes("本月")) ?? list[list.length - 1];
});

/** 其余窗口（滚动 / 本周…）压成一行小字，不抢主体。 */
const minorWindows = computed(() => quotaWindows.value.filter((w) => w !== monthWindow.value));

/** 本月剩余低于 20% 时进度条与数字转为警示色。 */
const monthLow = computed(() => (monthWindow.value?.remainingPercent ?? 100) < 20);

/** 进度条宽度（夹在 0–100，防后端给出越界值）。 */
const barWidth = computed(() => {
  const v = monthWindow.value?.remainingPercent ?? 0;
  return `${Math.max(0, Math.min(100, v))}%`;
});

/** 已连接的蓝牙设备（Rust 侧已按"音频优先"排好序）。 */
const devices = computed(() => {
  const bt = snapshot.value.bluetooth;
  return bt.status === "ok" ? bt.devices : [];
});

/** 窗口重置时刻的人话（解析失败原样显示，绝不显示 Invalid Date）。 */
function resetText(iso: string | null): string {
  if (!iso) return "重置未知";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return `${date.toLocaleString("zh-CN", {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  })} 重置`;
}

/** 完整设备名（悬停提示用）。 */
function devTitle(name: string, percent: number | null): string {
  return `${name}${percent === null ? " · 电量未知" : ` · ${percent}%`}`;
}

/**
 * 最近一次刷新失败（数值是保留的旧值）。
 *
 * 悬浮窗是**主窗口最小化后唯一还在的界面**，所以"这个数字有多旧"必须
 * 在这里说全 —— 状态栏还能靠 title 悬停解释，浮窗的信息得自己站得住。
 */
const quotaIsStale = computed(() => quotaStale.value !== null);
const quotaStaleAge = computed(() => ageLabel(snapshot.value.quotaAtMs));
/** 失败原因（"网络不可用" / "接口异常：…"）—— 与状态栏同源，都出自 quotaLine。 */
const quotaStaleReason = computed(() => quotaStale.value?.message ?? "");
</script>

<template>
  <div class="overlay">
    <!-- ── 主体：本月额度 ────────────────────────────────────── -->
    <section class="card quota" :class="[snapshot.quota.status, { stale: quotaIsStale }]">
      <header class="q-head">
        <span class="q-label">本月额度</span>
        <span v-if="monthWindow" class="q-reset">{{ resetText(monthWindow.resetsAt) }}</span>
      </header>

      <template v-if="monthWindow">
        <p class="q-value" :class="{ low: monthLow }">
          {{ Math.round(monthWindow.remainingPercent) }}<i>%</i>
        </p>
        <div class="q-track">
          <i class="q-fill" :class="{ low: monthLow }" :style="{ width: barWidth }" />
        </div>
        <!-- 其余窗口：一行小字，滚动/本周各自的剩余 -->
        <Transition name="fade">
          <p v-if="minorWindows.length" class="q-minor">
            <span v-for="win in minorWindows" :key="win.label">
              {{ win.label }}
              <i>{{ Math.round(win.remainingPercent) }}%</i>
            </span>
          </p>
        </Transition>
        <!-- 软失败：数字保留，把"多旧 + 为什么没更新"补在下面 -->
        <p v-if="quotaIsStale" class="q-stale">
          <i class="q-dot" aria-hidden="true" />未更新 · {{ quotaStaleAge }}的数据（{{
            quotaStaleReason
          }}）
        </p>
      </template>

      <!-- 非多窗口接口（或未配置 / 出错）：退回一句话 -->
      <p v-else class="q-line">{{ quotaLine(snapshot.quota) }}</p>
    </section>

    <!-- ── 底部：蓝牙设备（图标 + 短名 + 电量，尽量小） ──────── -->
    <TransitionGroup v-if="devices.length" name="dev" tag="footer" class="devs">
      <span
        v-for="dev in devices"
        :key="dev.id"
        class="dev"
        :title="devTitle(dev.name, dev.batteryPercent)"
      >
        <svg class="dev-ic" viewBox="0 0 24 24" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <template v-if="dev.category === 'audio'">
            <path d="M3 14h3a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z" />
            <path d="M21 14h-3a1 1 0 0 0-1 1v3a1 1 0 0 0 1 1h2a1 1 0 0 0 1-1z" />
            <path d="M3 14v-2a9 9 0 0 1 18 0v2" />
          </template>
          <template v-else-if="dev.category === 'keyboard'">
            <rect x="2" y="6" width="20" height="12" rx="2" />
            <path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10" />
          </template>
          <template v-else-if="dev.category === 'mouse'">
            <rect x="7" y="2" width="10" height="20" rx="5" />
            <path d="M12 6v4" />
          </template>
          <template v-else>
            <path d="m7 7 10 10-5 5V2l5 5L7 17" />
          </template>
        </svg>
        <span class="dev-nm">{{ shortName(dev.name) }}</span>
        <i class="dev-bat" :class="{ unknown: dev.batteryPercent === null }">
          {{ batteryText(dev.batteryPercent) }}
        </i>
      </span>
    </TransitionGroup>
    <footer v-else class="devs empty">
      <span class="dev-none">{{ deviceLine(snapshot.bluetooth) }}</span>
    </footer>
  </div>
</template>

<style scoped>
/* 整窗即卡片：透明窗口，玻璃圆角自己画 */
.overlay {
  position: fixed;
  inset: 0;
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  padding: 0.5rem;
  border: 1px solid var(--glass-border);
  border-radius: 16px;
  background: color-mix(in srgb, var(--glass-bg-deep) 92%, transparent);
  backdrop-filter: blur(20px) saturate(1.4);
  box-shadow: var(--glass-edge), 0 10px 28px rgb(0 0 0 / 0.4);
  color: var(--ink-soft);
  font-size: 0.74rem;
  user-select: none;
  overflow: hidden;
}

/* 每个信息块都是一枚圆角玻璃卡（与主界面「开始」按钮同款变量） */
.card {
  border: 1px solid var(--glass-border);
  border-radius: 12px;
  background: var(--glass-bg);
  backdrop-filter: var(--glass-blur);
}

/* ------------------------------------------------------- 主体：额度 */
.quota {
  flex: 1;
  min-height: 0;
  padding: 0.42rem 0.6rem 0.5rem;
  display: flex;
  flex-direction: column;
  gap: 0.16rem;
}
.quota.warn {
  border-color: color-mix(in srgb, #e8b64c 40%, var(--glass-border));
}
.quota.err {
  border-color: color-mix(in srgb, #ff8a8a 40%, var(--glass-border));
}
.q-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
}
.q-label {
  font-size: 0.66rem;
  letter-spacing: 0.08em;
  color: var(--ink-dim);
}
.q-reset {
  font-size: 0.62rem;
  color: var(--ink-faint);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.q-value {
  margin: 0;
  font-size: 1.72rem;
  line-height: 1.05;
  font-weight: 300;
  font-variant-numeric: tabular-nums;
  letter-spacing: -0.02em;
  color: var(--ink-soft);
  transition: color var(--t-base) ease;
}
.q-value i {
  font-style: normal;
  font-size: 0.9rem;
  margin-left: 0.08rem;
  color: var(--ink-faint);
}
/* 剩余 < 20% 转警示色 */
.q-value.low {
  color: #e8b64c;
}
.q-track {
  position: relative;
  height: 5px;
  border-radius: 999px;
  /* 底槽用主题相对的叠层：写死白色在浅色主题下会隐形 */
  background: color-mix(in srgb, var(--ink) 14%, transparent);
  overflow: hidden;
}
.q-fill {
  position: absolute;
  inset: 0 auto 0 0;
  border-radius: 999px;
  background: var(--accent);
  /* ★ 宽度过渡：刷新后能直观看清"这一轮掉了多少" */
  transition: width 0.5s cubic-bezier(0.22, 1, 0.36, 1), background var(--t-base) ease;
}
.q-fill.low {
  background: #e8b64c;
}
.q-minor {
  display: flex;
  gap: 0.6rem;
  margin: 0.1rem 0 0;
  font-size: 0.62rem;
  color: var(--ink-faint);
}
.q-minor i {
  font-style: normal;
  font-variant-numeric: tabular-nums;
  color: var(--ink-dim);
}
/* 软失败（"未更新"）：数值保留、卡片不上告警边框，只把标题与数字降一档，
   再用一行小字说清"多旧 + 为什么"。 */
.quota.stale .q-value {
  color: var(--ink-dim);
}
.q-stale {
  display: flex;
  align-items: center;
  gap: 0.28rem;
  margin: 0.12rem 0 0;
  font-size: 0.6rem;
  line-height: 1.3;
  /* 这一行是浮窗上"数据是旧的"的**唯一**解释，比同级的次要信息
     （.q-minor 用 --ink-faint）再提一档，别让它自身也看不清。 */
  color: var(--ink-dim);
}
.q-dot {
  flex: none;
  width: 4px;
  height: 4px;
  border-radius: 50%;
  background: color-mix(in srgb, var(--ink) 46%, transparent);
}
.q-line {
  margin: 0.2rem 0 0;
  font-size: 0.7rem;
  color: var(--ink-dim);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ------------------------------------------------------- 底部：设备 */
.devs {
  display: flex;
  flex-wrap: wrap;
  gap: 0.3rem;
  padding: 0 0.1rem;
}
.devs.empty {
  padding: 0.16rem 0.2rem 0.04rem;
}
.dev {
  display: inline-flex;
  align-items: center;
  gap: 0.22rem;
  max-width: 100%;
  padding: 0.1rem 0.4rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  background: var(--glass-bg);
  font-size: 0.64rem;
  color: var(--ink-dim);
}
.dev-ic {
  flex: none;
  color: var(--ink-faint);
}
.dev-nm {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--ink-soft);
}
.dev-bat {
  flex: none;
  font-style: normal;
  font-variant-numeric: tabular-nums;
  color: var(--accent);
}
.dev-bat.unknown {
  color: var(--ink-faint);
}
.dev-none {
  font-size: 0.64rem;
  color: var(--ink-faint);
}

/* ------------------------------------------------------- 过渡动画 */
/* 设备增减：胶囊从下往上淡入，退场反向 */
.dev-enter-active,
.dev-leave-active {
  transition:
    opacity 0.28s ease,
    transform 0.28s cubic-bezier(0.22, 1, 0.36, 1);
}
.dev-enter-from,
.dev-leave-to {
  opacity: 0;
  transform: translateY(4px) scale(0.96);
}
.dev-leave-active {
  position: absolute;
}

/* 次要窗口行的显隐 */
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.25s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>

<style>
/* 覆盖全局的 <html> 背景（glass.css 给 html 画了渐变底色）：
   悬浮窗是透明窗口，页面背景必须透明，玻璃卡片才有意义。
   这个选择器只会在 overlay 窗口命中（main.ts 挂载前加了这个类）。 */
html.overlay-mode {
  background: transparent !important;
}
</style>
