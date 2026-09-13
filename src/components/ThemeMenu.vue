<script setup lang="ts">
// ---------------------------------------------------------------------------
// 底部主题菜单：按钮显示当前主题（色点 + 名称），点击向上弹出玻璃菜单，
// 六主题带选中态；外点 / Esc 关闭。第 21 轮引入，P1-⑤ 第二步自 App.vue 拆出。
//
// 主题状态来自 useTheme 模块级单例（与 App 的 SmokeField 共享同一份 ref）；
// 开合态是本组件私有的纯 UI 态 —— 所以全局外点/键盘监听也随组件挂卸。
// ---------------------------------------------------------------------------

import { onMounted, onUnmounted, ref } from "vue";
import { useTheme } from "../composables/useTheme";

const { THEMES, theme, currentThemeLabel, currentThemeSwatch, pickTheme } = useTheme();

const open = ref(false);
const root = ref<HTMLElement | null>(null);

function onPointerDown(e: PointerEvent): void {
  if (!open.value) return;
  if (root.value && !root.value.contains(e.target as Node)) {
    open.value = false;
  }
}

function onKeydown(e: KeyboardEvent): void {
  if (e.key === "Escape") open.value = false;
}

onMounted(() => {
  document.addEventListener("pointerdown", onPointerDown);
  document.addEventListener("keydown", onKeydown);
});

onUnmounted(() => {
  document.removeEventListener("pointerdown", onPointerDown);
  document.removeEventListener("keydown", onKeydown);
});

function pick(id: (typeof THEMES)[number]["id"]): void {
  pickTheme(id);
  open.value = false;
}
</script>

<template>
  <span ref="root" class="theme-ctl">
    <button
      class="theme-btn"
      :class="{ open }"
      :aria-expanded="open"
      title="切换主题"
      @click="open = !open"
    >
      <i class="theme-dot" :style="{ background: currentThemeSwatch }" />
      <span>{{ currentThemeLabel }}</span>
      <svg class="chev" viewBox="0 0 10 6" aria-hidden="true">
        <path d="M1 1l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    </button>
    <Transition name="theme-pop">
      <span v-if="open" class="theme-menu" role="radiogroup" aria-label="背景主题">
        <button
          v-for="t in THEMES"
          :key="t.id"
          class="theme-item"
          :class="{ active: theme === t.id }"
          role="radio"
          :aria-checked="theme === t.id"
          @click="pick(t.id)"
        >
          <i class="sw" :style="{ background: t.swatch }" />
          <span>{{ t.label }}</span>
          <i v-if="theme === t.id" class="check" aria-hidden="true">✓</i>
        </button>
      </span>
    </Transition>
  </span>
</template>

<style scoped>
.theme-ctl {
  position: relative;
  display: inline-flex;
  align-items: center;
}
/* 主题菜单按钮 —— 当前主题的色点 + 名称 + 下翻箭头，
   形态借 presets 按钮的胶囊壳，是底部菜单的一等公民而非角落色点 */
.theme-btn {
  display: inline-flex;
  align-items: center;
  gap: 0.45rem;
  padding: 0.3rem 0.65rem 0.3rem 0.45rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  background: var(--glass-bg);
  color: var(--ink-soft);
  font-size: 0.72rem;
  cursor: pointer;
  transition:
    border-color var(--t-base) var(--ease-out-expo),
    background var(--t-base) var(--ease-out-expo),
    color var(--t-base) ease,
    box-shadow var(--t-base) ease;
}
.theme-btn:hover,
.theme-btn.open {
  border-color: var(--glass-border-strong);
  background: var(--glass-bg-strong);
  color: var(--ink);
  box-shadow: var(--glass-shadow);
}
.theme-dot {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  border: 1px solid rgb(255 255 255 / 0.25);
  box-shadow: 0 0 8px color-mix(in srgb, var(--accent) 24%, transparent);
}
.theme-btn .chev {
  width: 9px;
  height: 6px;
  color: var(--ink-faint);
  transition: transform var(--t-base) var(--ease-out-back);
}
.theme-btn.open .chev {
  transform: rotate(180deg);
}
/* 弹出菜单：向上展开的玻璃面板，铺在底部菜单上方（高于内容层） */
.theme-menu {
  position: absolute;
  bottom: calc(100% + 10px);
  right: 0;
  z-index: var(--z-chrome);
  display: flex;
  flex-direction: column;
  min-width: 132px;
  padding: 4px;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius);
  background: var(--glass-bg-strong);
  backdrop-filter: var(--glass-blur);
  box-shadow: var(--glass-edge-strong), var(--glass-shadow-lg);
  transform-origin: 85% 100%;
}
.theme-item {
  display: flex;
  align-items: center;
  gap: 0.55rem;
  padding: 0.42rem 0.6rem;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--ink-soft);
  font-size: 0.74rem;
  text-align: left;
  cursor: pointer;
  transition: background var(--t-fast) ease, color var(--t-fast) ease;
}
.theme-item:hover {
  background: color-mix(in srgb, var(--ink) 8%, transparent);
  color: var(--ink);
}
/* ★ 选中项不能直接吃 --accent：空闲态 accent 是深灰 #4a5160，
   深色主题下文字会隐形 —— 文字用墨色、选中态用浅色底表达。
   ★ 特异性警示：元素同时命中 .presets button.active（0,3,1），
   这里必须挂 .presets 前缀抬到 (0,4,0) 才能盖过它。
   （scoped 只把属性挂在最后一个选择器上，祖先段 .presets 不受影响。） */
.presets .theme-item.active {
  color: var(--ink);
  background: color-mix(in srgb, var(--ink) 10%, transparent);
}
.theme-item .sw {
  width: 15px;
  height: 15px;
  flex: none;
  border-radius: 50%;
  border: 1px solid var(--glass-border-strong);
}
.theme-item .check {
  margin-left: auto;
  font-size: 0.68rem;
}
/* 弹出过渡：从按钮锚点浮起 + 回弹收尾（--ease-out-back 的微过冲） */
.theme-pop-enter-active {
  transition: opacity 0.26s var(--ease-out-expo), transform 0.3s var(--ease-out-back);
}
.theme-pop-leave-active {
  transition: opacity 0.18s ease, transform 0.18s ease;
}
.theme-pop-enter-from,
.theme-pop-leave-to {
  opacity: 0;
  transform: translateY(8px) scale(0.94);
}

/* 动效偏好减弱：弹出直接呈现 */
@media (prefers-reduced-motion: reduce) {
  .theme-pop-enter-active,
  .theme-pop-leave-active {
    transition: none;
  }
}
</style>
