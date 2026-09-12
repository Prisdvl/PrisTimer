<script setup lang="ts">
// ---------------------------------------------------------------------------
// 滑块开关（参考集 Toggle）：弹性滑动 + 激活发光。
// 轨道 46×24，旋钮 cubic-bezier(0.34,1.56,0.64,1) 回弹 —— 设置项通用控件。
// ---------------------------------------------------------------------------

defineProps<{ modelValue: boolean; label?: string }>();
const emit = defineEmits<{ "update:modelValue": [value: boolean] }>();
</script>

<template>
  <button
    type="button"
    role="switch"
    class="toggle"
    :class="{ on: modelValue }"
    :aria-checked="modelValue"
    :aria-label="label"
    @click="emit('update:modelValue', !modelValue)"
  >
    <span class="knob" />
  </button>
</template>

<style scoped>
.toggle {
  position: relative;
  flex: none;
  width: 46px;
  height: 24px;
  padding: 0;
  border: none;
  border-radius: 12px;
  background: rgb(255 255 255 / 0.1);
  box-shadow: inset 0 2px 6px rgb(0 0 0 / 0.3);
  cursor: pointer;
  transition: background var(--t-base) ease;
}
.toggle.on {
  background: color-mix(in srgb, var(--accent) 55%, transparent);
}
.knob {
  position: absolute;
  top: 3px;
  left: 3px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 2px 8px rgb(0 0 0 / 0.35);
  transition: all 0.35s var(--ease-out-back);
}
.toggle.on .knob {
  left: 25px;
  background: var(--accent);
}
.toggle.on .knob::after {
  content: "";
  position: absolute;
  inset: 0;
  margin: auto;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 0 10px #fff;
}
.toggle:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
</style>
