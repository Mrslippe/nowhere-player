import { describe, expect, it } from "vitest";

import { readLibraryStats } from "./library-bridge";

const VALID = {
  tracks: 1284,
  albums: 96,
  artists: 41,
  totalDurationMs: 4_500_000,
  lastScanAt: 1_760_000_000,
};

describe("readLibraryStats", () => {
  it("接受完整且非负的 Stats", () => {
    expect(readLibraryStats(VALID)).toEqual(VALID);
    expect(readLibraryStats({ ...VALID, tracks: 0, lastScanAt: 0 })).toEqual({
      ...VALID,
      tracks: 0,
      lastScanAt: 0,
    });
  });

  it("缺字段 / 负值 / 非数字一律回 null", () => {
    for (const value of [
      null,
      undefined,
      "stats",
      {},
      { ...VALID, tracks: undefined },
      { ...VALID, albums: -1 },
      { ...VALID, artists: Number.NaN },
      { ...VALID, totalDurationMs: "4500000" },
      { ...VALID, lastScanAt: Number.POSITIVE_INFINITY },
    ]) {
      expect(readLibraryStats(value)).toBeNull();
    }
  });
});
