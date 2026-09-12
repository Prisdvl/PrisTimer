import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { ref, nextTick } from "vue";
import { useCountUp } from "../composables/useCountUp";

/** rAF 桩：手动推进帧队列，返回「推进一帧」函数与调用记录。 */
function stubRaf() {
  const calls: number[] = [];
  let now = 0;
  let queue: Array<(t: number) => void> = [];
  vi.stubGlobal("performance", { now: () => now });
  vi.stubGlobal("requestAnimationFrame", (cb: (t: number) => void) => {
    queue.push(cb);
    return queue.length;
  });
  vi.stubGlobal("cancelAnimationFrame", () => {
    queue = [];
  });
  /** 推进 n 帧，每帧步进 stepMs */
  const flush = (n: number, stepMs = 120) => {
    for (let i = 0; i < n; i += 1) {
      now += stepMs;
      const q = queue;
      queue = [];
      q.forEach((cb) => cb(now));
    }
  };
  return { calls, flush };
}

beforeEach(() => {
  vi.unstubAllGlobals();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("useCountUp", () => {
  it("初始文本 = format(0)", () => {
    const src = ref(0);
    const text = useCountUp(src, (v) => `${Math.round(v)}`);
    expect(text.value).toBe("0");
  });

  it("epsilon 内的变化跳过动画直接赋值", async () => {
    const raf = vi.fn();
    vi.stubGlobal("requestAnimationFrame", raf);
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
    const src = ref(100_000);
    // 显式 epsilon=200_000：100k → 125k 差值在阈值内，必须直赋
    const text = useCountUp(src, (v) => `${Math.round(v)}`, 1000, 200_000);
    await nextTick();
    src.value = 125_000;
    await nextTick();
    expect(text.value).toBe("125000");
    expect(raf).not.toHaveBeenCalled();
  });

  it("大变化走 rAF 补间，最终收敛到目标值", async () => {
    const { flush } = stubRaf();
    const src = ref(0);
    const text = useCountUp(src, (v) => `${Math.round(v / 60_000)} 分钟`);
    await nextTick();
    src.value = 120_000; // 0 → 120s，超出 epsilon
    await nextTick();
    flush(30);
    expect(text.value).toBe("2 分钟");
  });

  it("格式化后文本不变时不写 ref（中间帧取整同值）", async () => {
    const { flush } = stubRaf();
    const src = ref(0);
    // 取整到小时的格式：中间帧几乎全是 "0 小时"
    const text = useCountUp(src, (v) => `${Math.floor(v / 3_600_000)} 小时`, 1000, 1000);
    await nextTick();
    const seen = new Set<string>();
    // 采集中间帧：手动模拟 10 帧，记录写入次数 —— 中间帧文本高度重复
    src.value = 7_200_000; // 0 → 2 小时
    await nextTick();
    flush(2, 100);
    seen.add(text.value);
    flush(3, 100);
    seen.add(text.value);
    flush(10, 200);
    seen.add(text.value);
    expect(text.value).toBe("2 小时");
    expect(seen.has("2 小时")).toBe(true);
  });
});
