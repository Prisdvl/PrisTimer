import { describe, expect, it } from "vitest";
import { addDays, formatClock, formatDuration, heatLevel, toDayKey } from "../date";

describe("toDayKey", () => {
  it("按本地时区拼 YYYY-MM-DD，月/日补零", () => {
    // 用本地时间构造（避开 toISOString 的 UTC 坑——这正是被测函数存在的理由）
    expect(toDayKey(new Date(2026, 8, 12))).toBe("2026-09-12");
    expect(toDayKey(new Date(2026, 0, 3))).toBe("2026-01-03");
    expect(toDayKey(new Date(2026, 11, 31))).toBe("2026-12-31");
  });
});

describe("addDays", () => {
  it("跨月/跨年进位正确", () => {
    expect(toDayKey(addDays(new Date(2026, 0, 31), 1))).toBe("2026-02-01");
    expect(toDayKey(addDays(new Date(2026, 11, 31), 1))).toBe("2027-01-01");
    expect(toDayKey(addDays(new Date(2026, 2, 1), -1))).toBe("2026-02-28");
  });

  it("不修改入参", () => {
    const base = new Date(2026, 8, 12);
    addDays(base, 7);
    expect(toDayKey(base)).toBe("2026-09-12");
  });
});

describe("formatClock", () => {
  it("mm:ss 与 h:mm:ss 两档", () => {
    expect(formatClock(0)).toBe("00:00");
    expect(formatClock(59_000)).toBe("00:59");
    expect(formatClock(61_000)).toBe("01:01");
    expect(formatClock(3_600_000)).toBe("1:00:00");
    expect(formatClock(3_661_000)).toBe("1:01:01");
  });

  it("负数与亚秒封底", () => {
    expect(formatClock(-5)).toBe("00:00");
    expect(formatClock(999)).toBe("00:00");
  });
});

describe("formatDuration", () => {
  it("分钟/小时两档文案", () => {
    expect(formatDuration(0)).toBe("0 分钟");
    expect(formatDuration(29 * 60_000)).toBe("29 分钟");
    expect(formatDuration(60 * 60_000)).toBe("1 小时");
    expect(formatDuration(90 * 60_000)).toBe("1 小时 30 分");
  });

  it("四舍五入到分钟", () => {
    // 90 秒 ≈ 1.5 分钟 → round 到 2
    expect(formatDuration(90_000)).toBe("2 分钟");
    // 59 分 40 秒 → round 到 60 分 → 1 小时
    expect(formatDuration(59 * 60_000 + 40_000)).toBe("1 小时");
  });
});

describe("heatLevel", () => {
  it("绝对阈值 0-4 档", () => {
    expect(heatLevel(0)).toBe(0);
    expect(heatLevel(-1)).toBe(0);
    expect(heatLevel(60_000)).toBe(1); // 1 分钟
    expect(heatLevel(24.9 * 60_000)).toBe(1);
    expect(heatLevel(25 * 60_000)).toBe(2); // 一个番茄
    expect(heatLevel(59.9 * 60_000)).toBe(2);
    expect(heatLevel(60 * 60_000)).toBe(3);
    expect(heatLevel(149.9 * 60_000)).toBe(3);
    expect(heatLevel(150 * 60_000)).toBe(4);
    expect(heatLevel(10 * 3600_000)).toBe(4);
  });
});
