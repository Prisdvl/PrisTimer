import { describe, expect, it } from "vitest";
import { planQuotaIngest } from "../composables/useInsight";
import type { QuotaStatus } from "../api";

// ---------------------------------------------------------------------------
// 额度落地规则（`planQuotaIngest`）
//
// 这组断言锁的是**语义**而不是实现：2026-09-17 用户拍板的取舍是
// "周期性查询的偶发失败不覆盖已有成功值，改为标记陈旧；用户显式动作的失败
//  仍如实落地"。这条规则住在 useInsight 里，是三个展示层（状态栏 / 迷你窗 /
// 悬浮窗）共同的行为前提 —— 它变了，三处的表现会一起变，所以值得钉住。
//
// 纯函数直接喂参数，不需要 DOM / Tauri 运行时。
// ---------------------------------------------------------------------------

/** 一个成功的额度状态（多窗口，贴近真实返回）。 */
const OK: QuotaStatus = {
  status: "ok",
  remaining: 74,
  total: null,
  used: null,
  unit: "%",
  resetsAt: null,
  raw: "",
  windows: [
    { label: "滚动", remainingPercent: 100, resetsAt: null, status: "ok" },
    { label: "本月", remainingPercent: 74, resetsAt: null, status: "ok" },
  ],
};

const NETWORK: QuotaStatus = { status: "network", message: "网络错误：12002" };
const API_ERROR: QuotaStatus = { status: "apiError", message: "接口返回 500" };
const UNAUTHORIZED: QuotaStatus = { status: "unauthorized", message: "401" };
const NOT_CONFIGURED: QuotaStatus = { status: "notConfigured" };

/** 5 分钟前拿到的成功值 —— 后端周期就是 5 分钟，这是最典型的"手里有旧值"。 */
const FIVE_MIN_AGO = Date.UTC(2026, 8, 17, 12, 0, 0);
const NOW = FIVE_MIN_AGO + 5 * 60_000;
const HELD = { quota: OK, atMs: FIVE_MIN_AGO };

describe("planQuotaIngest", () => {
  it("成功结果一律落地", () => {
    expect(planQuotaIngest(OK, HELD, { now: NOW })).toBe("apply");
  });

  it("周期性失败：手里有好值 → 保留（陈旧）", () => {
    expect(planQuotaIngest(NETWORK, HELD, { now: NOW })).toBe("hold");
    expect(planQuotaIngest(API_ERROR, HELD, { now: NOW })).toBe("hold");
  });

  it("用户显式动作（立即刷新 / 保存配置）→ 失败如实落地", () => {
    expect(planQuotaIngest(NETWORK, HELD, { user: true, now: NOW })).toBe("apply");
  });

  it("配置类与凭据类错误不被降级成「陈旧」", () => {
    // 未配置：藏着会让用户以为已经配好了。
    expect(planQuotaIngest(NOT_CONFIGURED, HELD, { now: NOW })).toBe("apply");
    // 401：凭据被拒是可行动的硬错误，不该只留一个暗色小标。
    expect(planQuotaIngest(UNAUTHORIZED, HELD, { now: NOW })).toBe("apply");
  });

  it("手里没有好值（首次查询就失败）→ 如实落地", () => {
    expect(planQuotaIngest(NETWORK, { quota: NOT_CONFIGURED, atMs: 0 }, { now: NOW })).toBe("apply");
    // 已被前一次失败覆盖过（quota 不是 ok）→ 也不能端着空值说"陈旧"
    expect(planQuotaIngest(API_ERROR, { quota: NETWORK, atMs: NOW - 60_000 }, { now: NOW })).toBe(
      "apply",
    );
  });

  it("旧值超过保留上限（30 分钟）→ 不再保留，如实报错", () => {
    const justUnder = { quota: OK, atMs: NOW - 29 * 60_000 };
    const atLimit = { quota: OK, atMs: NOW - 30 * 60_000 };
    expect(planQuotaIngest(NETWORK, justUnder, { now: NOW })).toBe("hold");
    // 边界取 >=：正好 30 分钟就不再保留
    expect(planQuotaIngest(NETWORK, atLimit, { now: NOW })).toBe("apply");
  });
});
