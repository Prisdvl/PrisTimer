<script setup lang="ts">
// ---------------------------------------------------------------------------
// 科目标签行：选中的科目会落到下一条会话上，进统计与 CSV。
// 标签本身可维护 —— 双击重命名，悬停出现 × 删除，输入框回车新增。
//
// P1-⑤ 第二步自 App.vue 拆出：状态全部来自 useTags 模块级单例
// （与 App 的迷你组件选中态共享同一份 ref），无 props。
// ---------------------------------------------------------------------------

import { useTags } from "../composables/useTags";

const {
  tags,
  selectedTag,
  tagInput,
  editingTag,
  editingText,
  applyTag,
  submitTag,
  startTagEdit,
  cancelTagEdit,
  commitTagEdit,
  removeTag,
} = useTags();
</script>

<template>
  <div class="tags">
    <div v-for="tag in tags" :key="tag" class="tag-slot">
      <input
        v-if="editingTag === tag"
        v-model="editingText"
        class="tag-edit"
        type="text"
        maxlength="20"
        @keyup.enter="commitTagEdit"
        @keyup.esc="cancelTagEdit"
        @blur="commitTagEdit"
      />
      <button
        v-else
        class="tag-chip"
        :class="{ active: selectedTag === tag }"
        :title="`${tag} · 双击重命名`"
        @click="applyTag(selectedTag === tag ? null : tag)"
        @dblclick="startTagEdit(tag)"
      >
        <span class="chip-text">{{ tag }}</span>
        <span class="chip-del" title="删除这个标签" @click.stop="removeTag(tag)">×</span>
      </button>
    </div>
    <input
      v-model="tagInput"
      class="tag-input"
      type="text"
      maxlength="20"
      placeholder="自定义"
      title="输入科目后按回车添加"
      @keyup.enter="submitTag"
    />
  </div>
</template>

<style scoped>
.tags {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-wrap: wrap;
  gap: 0.45rem;
}
/* 每个标签占一个槽：输入态与展示态在同一位置切换，不会把整行挤动 */
.tag-slot {
  display: inline-flex;
}
.tag-chip {
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
  /* 右侧多留一点：删除键常驻占位（只是透明），悬停时出现不会引起位移 */
  padding: 0.22rem 0.4rem 0.22rem 0.85rem;
  max-width: 11rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  background: var(--glass-bg);
  backdrop-filter: var(--glass-blur);
  color: var(--ink-dim);
  font: inherit;
  font-size: 0.75rem;
  cursor: pointer;
  transition:
    color var(--t-base) var(--ease-out-expo),
    border-color var(--t-base) var(--ease-out-expo),
    background var(--t-base) var(--ease-out-expo),
    box-shadow var(--t-base) var(--ease-out-expo),
    transform var(--t-base) var(--ease-out-back);
}
.tag-chip:hover {
  color: var(--ink-soft);
  border-color: var(--glass-border-strong);
  transform: translateY(-1px);
}
.tag-chip:active {
  transform: scale(0.95);
}
.tag-chip.active {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 16%, transparent);
  color: var(--accent);
  box-shadow: 0 0 12px color-mix(in srgb, var(--accent) 26%, transparent);
}
/* 自定义标签可能很长（上限 20 字）：文字截断而不是把整行撑爆 */
.chip-text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* 删除键：平时透明占位，悬停整颗胶囊时浮现 */
.chip-del {
  flex: none;
  display: grid;
  place-items: center;
  width: 1.1em;
  height: 1.1em;
  border-radius: 50%;
  font-size: 1.1em;
  line-height: 1;
  opacity: 0;
  transform: scale(0.6);
  transition:
    opacity var(--t-fast) ease,
    transform var(--t-fast) var(--ease-out-back),
    background var(--t-fast) ease,
    color var(--t-fast) ease;
}
.tag-chip:hover .chip-del {
  opacity: 0.7;
  transform: none;
}
.chip-del:hover {
  opacity: 1;
  background: rgb(255 120 120 / 0.28);
  color: #ffd0d0;
}
/* 内联重命名的输入框：与胶囊同高同形，切换上去像是同一颗控件换了状态 */
.tag-edit {
  width: 7em;
  padding: 0.22rem 0.7rem;
  border: 1px solid var(--accent);
  border-radius: 999px;
  background: var(--glass-bg-strong);
  color: var(--ink);
  font: inherit;
  font-size: 0.75rem;
  text-align: center;
  outline: none;
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
}
/* ★ 宽度必须放得下 placeholder「自定义」三个汉字。
   原来是 5ch —— ch 是数字 "0" 的宽度（0.75rem 下约 6.7px），5ch ≈ 33px，
   再扣掉左右 padding 就只剩十几个像素，三个汉字（36px）根本显示不全。
   改用 em（1em = 一个汉字宽）来对齐意图。 */
.tag-input {
  width: 5em;
  padding: 0.22rem 0.5rem;
  border: 1px dashed rgb(255 255 255 / 0.18);
  border-radius: 999px;
  background: transparent;
  color: var(--ink-soft);
  font: inherit;
  font-size: 0.75rem;
  text-align: center;
  transition:
    width var(--t-base) var(--ease-out-expo),
    border-color var(--t-base) ease,
    color var(--t-base) ease,
    background var(--t-base) ease;
}
.tag-input::placeholder {
  color: #565d68;
}
.tag-input:focus {
  outline: none;
  border-style: solid;
  border-color: var(--accent);
  width: 9em;
  background: var(--glass-bg);
}

/* 动效偏好减弱：胶囊直接呈现 */
@media (prefers-reduced-motion: reduce) {
  .tag-chip {
    transition: none;
  }
}
</style>
