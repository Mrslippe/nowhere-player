import { describe, expect, it } from "vitest";
import { formatTime } from "./format";

describe("formatTime", () => {
  it.each([
    [0, "00:00"],
    [59_000, "00:59"],
    [60_000, "01:00"],
    [61_500, "01:01"],
    [3_600_000, "1:00:00"],
    [3_690_000, "1:01:30"],
    [3_723_000, "1:02:03"],
  ])("%i ms → %s", (ms, expected) => {
    expect(formatTime(ms)).toBe(expected);
  });

  it("负数、NaN、Infinity 统一回退 00:00", () => {
    expect(formatTime(-1)).toBe("00:00");
    expect(formatTime(Number.NaN)).toBe("00:00");
    expect(formatTime(Number.POSITIVE_INFINITY)).toBe("00:00");
  });

  it("毫秒向下取整，不四舍五入", () => {
    expect(formatTime(59_999)).toBe("00:59");
    expect(formatTime(60_999)).toBe("01:00");
  });
});
