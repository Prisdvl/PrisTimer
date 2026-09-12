import { describe, expect, it } from "vitest";
import { TAG_PRESETS } from "../tags";

describe("TAG_PRESETS", () => {
  it("默认四科齐全", () => {
    expect(TAG_PRESETS).toEqual(["高数", "线代", "英语", "专业课"]);
  });

  it("无重复（计时页与统计页共用，重复会渲染两枚同名片）", () => {
    expect(new Set(TAG_PRESETS).size).toBe(TAG_PRESETS.length);
  });
});
