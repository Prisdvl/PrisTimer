// ---------------------------------------------------------------------------
// 数字补间：源值变化时，用 rAF 从旧值滚到新值。
//
// 这就是 GSAP tween 的最小内核：每帧根据缓动函数插值。缓动曲线取自
// easings.net 的 easeOutQuart —— 比 expo 起步平缓，对"分钟"这类粗粒度
// 取值单元，观感是连续滚动而不是前几次帧的大跳变（expo 会闪）。
// 不引 GSAP 的原因：零依赖即可覆盖，省 30KB+ 运行时。
// ---------------------------------------------------------------------------

import { ref, watch, onUnmounted, type Ref } from "vue";

/** easings.net #easeOutQuart：1 - (1-t)^4 */
function easeOutQuart(t: number): number {
  return 1 - Math.pow(1 - t, 4);
}

/**
 * 跟随 `source` 的补间文本。`format` 决定展示形态（如 formatDuration）。
 *
 * `epsilon`：变化量低于该值时跳过动画直接赋值 —— 对"取整到分钟"的展示，
 * 30 秒内的变化不可见，为它滚 1 秒反而像闪烁。
 */
export function useCountUp(
  source: Ref<number>,
  format: (value: number) => string,
  duration = 1000,
  epsilon = 30_000,
): Ref<string> {
  const text = ref(format(0));
  let raf = 0;
  let current = 0;

  watch(source, (target) => {
    cancelAnimationFrame(raf);
    const from = current;
    if (from === target || Math.abs(target - from) <= epsilon) {
      current = target;
      text.value = format(target);
      return;
    }
    const start = performance.now();
    const step = (now: number): void => {
      const t = Math.min(1, (now - start) / duration);
      current = from + (target - from) * easeOutQuart(t);
      const next = format(current);
      // 格式化后的文本没变就不写 ref，省掉无意义的重渲染。
      if (next !== text.value) text.value = next;
      if (t < 1) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
  });

  onUnmounted(() => cancelAnimationFrame(raf));
  return text;
}
