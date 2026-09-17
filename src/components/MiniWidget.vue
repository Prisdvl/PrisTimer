<script setup lang="ts">
// ---------------------------------------------------------------------------
// 迷你组件：整窗缩成一枚贴边置顶的"组件"。
//
// 版式（2026-09-17 第 29 轮重排，用户要求"电量与额度在最小化窗口也要显示"）：
//
//   ┌──────────────────────────────────┐
//   │ 00:00            🎧60%  🔋87%    │  ← 时间 + 环境信息
//   │ 空闲 算法             [▶][⤢][≡]  │  ← 状态 + 操作
//   └──────────────────────────────────┘
//
// 为什么环境信息放第一行右边：264px 宽塞不下"时钟 + 状态 + 多台设备 + 额度 + 三个
// 按钮"一整行，而第一行右侧本来是空白 —— 把设备/额度挪上去正好填掉那块白，
// 不必压缩字号也不必砍按钮。
//
// ★ 数据不走 props 而直接消费 `useInsight`（与 StatusBar 同款）：它是模块级单例，
//   两个窗口各自订阅 `insight:update`。计时相关的显示状态仍是 props（归 App 管），
//   环境信息归 useInsight —— 各管各的，不必让 App 中转一层。
//
// 整面可拖拽（data-tauri-drag-region），按钮不拖拽只响应点击；
// 底部细线是倒计时进度（与主表盘的外圈细弧同源）。
// ---------------------------------------------------------------------------

import { computed } from "vue";
import { timerApi } from "../api";
import { useInsight, batteryText, ageLabel } from "../composables/useInsight";

defineProps<{
  display: string;
  miniLabel: string;
  selectedTag: string | null;
  isRunning: boolean;
  /** 倒计时进度（0..1），无目标时为 null（细线隐藏）。 */
  progress: number | null;
}>();

const emit = defineEmits<{ toggle: []; tray: [] }>();

const { snapshot, quotaStale } = useInsight();

/** 已连接设备（Rust 侧已按"音频优先"排好序）。264px 宽最多容得下两台。 */
const devices = computed(() => {
  const bt = snapshot.value.bluetooth;
  return bt.status === "ok" ? bt.devices.slice(0, 2) : [];
});

/** 本月额度窗口（opencode Go 多窗口接口）；无数据 / 未配置 / 出错时为 null。 */
const monthWindow = computed(() => {
  const q = snapshot.value.quota;
  if (q.status !== "ok") return null;
  const list = q.windows ?? [];
  if (!list.length) return null;
  return list.find((w) => w.label.includes("本月")) ?? list[list.length - 1];
});

/** 额度剩余百分比：优先本月窗口；单值接口按 remaining/total 换算；拿不到给 null。 */
const quotaPct = computed(() => {
  const w = monthWindow.value;
  if (w) return Math.round(w.remainingPercent);
  const q = snapshot.value.quota;
  if (q.status === "ok" && q.total && q.total > 0) {
    return Math.round((q.remaining / q.total) * 100);
  }
  return null;
});

/** 剩余低于 20% 转警示色。 */
const quotaLow = computed(() => (quotaPct.value ?? 100) < 20);

/** 查询失败（凭据被拒 / 网络不可用 / 接口异常）—— 与"没配 Key"区分开。
 *
 *  ★ 注意它现在**只在真的没有值可用时**才会亮（2026-09-17 第 30 轮）：
 *    周期查询偶发失败时 useInsight 会保留上一次的成功值并标 `quotaStale`，
 *    那时 `snapshot.quota` 仍是 `ok`，红色不出现 —— 走 `.stale` 那条更轻的通道。
 *    红色专门留给"确实拿不到数"（首次查询就失败、凭据被拒、超过保留上限）。 */
const quotaBad = computed(() => {
  const s = snapshot.value.quota.status;
  return s === "unauthorized" || s === "network" || s === "apiError";
});

/** 最近一次刷新失败：数值是保留的旧值（软状态，见 useInsight）。 */
const quotaIsStale = computed(() => quotaStale.value !== null);

/** 悬停提示给全名 + 精确电量（胶囊里只放图标与数字）。 */
function devTitle(name: string, percent: number | null): string {
  return `${name}${percent === null ? " · 电量未知" : ` · ${percent}%`}`;
}

const quotaTitle = computed(() => {
  const q = snapshot.value.quota;
  if (q.status !== "ok") return "额度不可用";
  const base = monthWindow.value
    ? `本月额度剩余 ${quotaPct.value}%`
    : `额度剩余 ${quotaPct.value}%`;
  // 陈旧时把"数据多旧 + 为什么没更新"一并交代 —— 迷你窗里唯一能放解释的地方。
  if (quotaStale.value) {
    return `${base}（显示 ${ageLabel(snapshot.value.quotaAtMs)}的数据；最近一次刷新失败：${quotaStale.value.message}）`;
  }
  return base;
});
</script>

<template>
  <div class="mini" data-tauri-drag-region>
    <span
      v-if="progress !== null"
      class="mini-progress"
      :style="{ transform: `scaleX(${progress})` }"
    />

    <!-- ── 第一行：时间 + 环境信息（设备电量 / 额度） ─────────────── -->
    <div class="mini-row" data-tauri-drag-region>
      <p class="mini-clock" :class="{ run: isRunning }">{{ display }}</p>

      <span class="mini-info">
        <span
          v-for="dev in devices"
          :key="dev.id"
          class="chip"
          :title="devTitle(dev.name, dev.batteryPercent)"
        >
          <svg
            class="c-ic"
            viewBox="0 0 24 24"
            width="10"
            height="10"
            fill="none"
            stroke="currentColor"
            stroke-width="2.2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
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
          <i
            class="c-val"
            :class="{
              unknown: dev.batteryPercent === null,
              low: (dev.batteryPercent ?? 100) < 20,
            }"
          >
            {{ batteryText(dev.batteryPercent) }}
          </i>
        </span>

        <!-- 额度：电池图标 + 剩余百分比（与设备同形，靠图标区分语义） -->
        <span
          class="chip quota"
          :class="{ low: quotaLow, bad: quotaBad, stale: quotaIsStale }"
          :title="quotaTitle"
        >
          <svg
            class="c-ic"
            viewBox="0 0 24 24"
            width="10"
            height="10"
            fill="none"
            stroke="currentColor"
            stroke-width="2.2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <rect x="2" y="7" width="16" height="10" rx="2.5" />
            <path d="M21 10.5v3" />
          </svg>
          <i class="c-val">{{ quotaPct !== null ? `${quotaPct}%` : "—" }}</i>
          <!-- 「未更新」的一个像素级标记：整格降成灰调之外再给一点结构性提示 -->
          <i v-if="quotaIsStale" class="c-dot" aria-hidden="true" />
        </span>
      </span>
    </div>

    <!-- ── 第二行：状态 + 操作 ──────────────────────────────────── -->
    <div class="mini-row" data-tauri-drag-region>
      <p class="mini-state">
        <i class="dot" />{{ miniLabel }}
        <span v-if="selectedTag" class="mini-tag">{{ selectedTag }}</span>
      </p>

      <div class="mini-actions">
        <button
          class="mini-btn"
          :class="{ live: isRunning }"
          :title="isRunning ? '暂停' : '开始'"
          @click="isRunning ? timerApi.pause() : timerApi.start()"
        >
          <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
            <rect v-if="isRunning" x="2" y="1.5" width="3" height="9" rx="1" fill="currentColor" />
            <rect v-if="isRunning" x="7" y="1.5" width="3" height="9" rx="1" fill="currentColor" />
            <path v-else d="M3 1.6v8.8c0 .5.55.8.97.53l7-4.4a.62.62 0 0 0 0-1.06l-7-4.4A.62.62 0 0 0 3 1.6Z" fill="currentColor" />
          </svg>
        </button>
        <button class="mini-btn" title="还原窗口" @click="emit('toggle')">
          <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" fill="none">
            <path
              d="M1.2 4.4V1.2h3.2M10.8 4.4V1.2H7.6M1.2 7.6v3.2h3.2M10.8 7.6v3.2H7.6"
              stroke="currentColor"
              stroke-width="1.2"
              stroke-linecap="round"
            />
          </svg>
        </button>
        <button class="mini-btn" title="收进托盘（计时继续）" @click="emit('tray')">
          <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" fill="none">
            <path
              d="M2 2.5h8M2 6h8M2 9.5h5"
              stroke="currentColor"
              stroke-width="1.2"
              stroke-linecap="round"
            />
          </svg>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 整窗即组件：两行纵向排布，内容垂直居中。
   ★ 圆角 16px 完全由 CSS 决定 —— 迷你态在 Rust 侧设了 DWMWCP_DONOTROUND
   （DWM 只有 ROUND≈8px / ROUNDSMALL≈4px 两档，做不出这个圆度），
   窗口 transparent:true，border-radius 之外就是透明像素。
   与悬浮信息窗（OverlayPanel）同半径，两个"小窗"才是同一套语言。 */
.mini {
  position: fixed;
  inset: 0;
  z-index: var(--z-content);
  overflow: hidden;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 0.1rem;
  padding: 0 0.6rem 0 0.78rem;
  border-radius: 16px;
  background:
    linear-gradient(165deg, color-mix(in srgb, var(--ink) 6%, transparent), transparent 55%),
    var(--glass-bg-strong);
  backdrop-filter: var(--glass-blur-lg);
  user-select: none;
  animation: mini-in 0.32s var(--ease-out-expo) backwards;
}
/* 进场：从略小、略透明处"贴"出来，与窗口自身由大到小的收缩连成一件事 */
@keyframes mini-in {
  from {
    opacity: 0;
    transform: scale(0.9);
  }
  55% {
    opacity: 1;
  }
}

/* 第一行右侧的环境信息（设备 / 额度）。
   错峰淡入：内容"随后跟上"比整块同时出现更有层次。 */
.mini-info {
  display: flex;
  align-items: center;
  gap: 0.28rem;
  flex: none;
  animation: mini-rise 0.34s var(--ease-out-expo) 0.1s backwards;
}
.chip {
  display: inline-flex;
  align-items: center;
  gap: 0.16rem;
  padding: 0.08rem 0.32rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  background: var(--glass-bg);
  color: var(--ink-soft);
  font-size: 0.58rem;
  line-height: 1.5;
  white-space: nowrap;
}
.c-ic {
  flex: none;
  opacity: 0.75;
}
.c-val {
  font-style: normal;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.02em;
}
.c-val.unknown {
  opacity: 0.55;
}
.c-val.low {
  color: #e8b64c;
}
/* 额度：正常态用强调色（它才是这枚芯片上最想被扫到的一眼）；
   低额度转琥珀、查询失败转红 —— 与状态栏/悬浮窗的错误色同一套。 */
.chip.quota {
  color: var(--accent);
  border-color: color-mix(in srgb, var(--accent) 30%, var(--glass-border));
}
/* 软失败（"未更新"）：数值仍可信（上一次的成功结果），只是这次没问到 ——
   把强调色降成"旧值"的中性灰调，红色留给"确实没有值"。
   放在 .low 之前：数值真的低于 20% 时"快用完了"比"没更新"更该被看见。 */
.chip.quota.stale {
  color: var(--ink-dim);
  border-color: color-mix(in srgb, var(--ink) 18%, transparent);
}
.chip.quota.low {
  color: #e8b64c;
  border-color: color-mix(in srgb, #e8b64c 38%, var(--glass-border));
}
.chip.quota.bad {
  color: #ff8a8a;
  border-color: color-mix(in srgb, #ff8a8a 34%, var(--glass-border));
}
/* 未更新的小圆点：颜色随主题（浅色主题下自动变深） */
.c-dot {
  flex: none;
  width: 4px;
  height: 4px;
  margin-left: 0.06rem;
  border-radius: 50%;
  background: color-mix(in srgb, var(--ink) 46%, transparent);
}

.mini-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.4rem;
  min-width: 0;
}
/* 倒计时进度细线：与主表盘的外圈细弧同源（ARC_PROGRESS），
   让"还剩多少"在余光里也能读到 */
.mini-progress {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 2px;
  background: var(--accent);
  opacity: 0.8;
  transform-origin: left center;
  transition: transform 0.4s var(--ease-out-quart);
  pointer-events: none;
}
.mini-clock {
  margin: 0;
  font-size: 1.5rem;
  line-height: 1.15;
  font-weight: 200;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.03em;
  color: var(--ink-soft);
  transition: color 0.4s ease;
  animation: mini-rise 0.34s var(--ease-out-expo) backwards;
}
.mini-clock.run {
  color: var(--accent);
}
.mini-state {
  display: flex;
  align-items: center;
  gap: 0.32rem;
  margin: 0;
  min-width: 0;
  font-size: 0.58rem;
  letter-spacing: 0.18em;
  text-transform: uppercase;
  color: var(--ink-dim);
  animation: mini-rise 0.34s var(--ease-out-expo) 0.06s backwards;
}
.mini-tag {
  padding: 0.02rem 0.36rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  font-size: 0.54rem;
  letter-spacing: 0.08em;
  color: var(--ink-soft);
  background: var(--glass-bg);
}
.mini-actions {
  display: flex;
  gap: 0.3rem;
  flex: none;
  animation: mini-rise 0.34s var(--ease-out-expo) 0.14s backwards;
}
@keyframes mini-rise {
  from {
    opacity: 0;
    transform: translateY(5px);
  }
}
.mini-btn {
  display: grid;
  place-items: center;
  width: 26px;
  height: 26px;
  padding: 0;
  border: 1px solid color-mix(in srgb, var(--ink) 14%, transparent);
  border-radius: 8px;
  background: var(--lg-bg);
  box-shadow: var(--lg-shadow);
  color: var(--ink-soft);
  cursor: pointer;
  backdrop-filter: var(--lg-filter);
  transition:
    background var(--t-fast) ease,
    border-color var(--t-fast) ease,
    color var(--t-fast) ease,
    transform var(--t-fast) var(--ease-out-back);
}
.mini-btn:hover {
  background: var(--lg-bg-hover);
  border-color: color-mix(in srgb, var(--ink) 26%, transparent);
  color: var(--ink);
}
.mini-btn:active {
  transform: scale(0.94);
}
.mini-btn.live {
  border-color: color-mix(in srgb, var(--accent) 55%, transparent);
  background: color-mix(in srgb, var(--accent) 16%, transparent);
  color: var(--accent);
}

/* 动效偏好减弱：细线与错峰入场直接呈现 */
@media (prefers-reduced-motion: reduce) {
  .mini-progress {
    transition: none;
  }
  .mini,
  .mini-clock,
  .mini-info,
  .mini-state,
  .mini-actions {
    animation: none;
  }
}
</style>
