import { describe, expect, it } from "vitest";
import {
  buildExportPayload,
  sumRendererBytes,
  type ResourceSizeEntry,
} from "./components/export";
import {
  compareScores,
  DEFAULT_SCORES,
  formatScore,
  isComparisonScores,
  loadScores,
  parseStoredScores,
  saveScores,
  SCORE_DIMENSIONS,
  SCORE_STORAGE_KEY,
  weightedTotal,
  type ComparisonScores,
  type ScoreSet,
} from "./components/scoring";

function scoreSet(value: number): ScoreSet {
  return {
    visual: value,
    interaction: value,
    ime: value,
    performance: value,
    extensibility: value,
  };
}

function memoryStorage() {
  const data = new Map<string, string>();

  return {
    data,
    getItem: (key: string): string | null => data.get(key) ?? null,
    setItem: (key: string, value: string): void => {
      data.set(key, value);
    },
  };
}

describe("评分维度与权重（M0 任务清单 §3）", () => {
  it("五维权重严格是 30 / 25 / 20 / 15 / 10", () => {
    expect(SCORE_DIMENSIONS.map((dimension) => dimension.weight)).toEqual([
      30, 25, 20, 15, 10,
    ]);
    expect(SCORE_DIMENSIONS.reduce((sum, dimension) => sum + dimension.weight, 0)).toBe(
      100,
    );
  });

  it("加权总分 = Σ(得分 × 权重) ÷ 100", () => {
    const mixed: ScoreSet = {
      visual: 5,
      interaction: 4,
      ime: 3,
      performance: 2,
      extensibility: 1,
    };

    expect(weightedTotal(mixed)).toBe(3.5);
    expect(formatScore(weightedTotal(mixed))).toBe("3.50");
    expect(weightedTotal(scoreSet(5))).toBe(5);
    expect(weightedTotal(scoreSet(1))).toBe(1);
  });

  it("自评基线：A 结构化 4.30，B xterm 3.40", () => {
    const comparison = compareScores(DEFAULT_SCORES);

    expect(formatScore(comparison.totals.blocks)).toBe("4.30");
    expect(formatScore(comparison.totals.xterm)).toBe("3.40");
    expect(comparison.leader).toBe("blocks");
    expect(formatScore(comparison.margin)).toBe("0.90");
  });
});

describe("compareScores", () => {
  it("总分高的一方领先，差值为两位小数", () => {
    const scores: ComparisonScores = {
      blocks: scoreSet(5),
      xterm: scoreSet(1),
    };
    const comparison = compareScores(scores);

    expect(comparison.totals).toEqual({ blocks: 5, xterm: 1 });
    expect(comparison.leader).toBe("blocks");
    expect(formatScore(comparison.margin)).toBe("4.00");
  });

  it("同分时判为持平", () => {
    const comparison = compareScores({
      blocks: scoreSet(3),
      xterm: scoreSet(3),
    });

    expect(comparison.leader).toBe("tie");
    expect(comparison.margin).toBe(0);
  });
});

describe("localStorage 持久化（key: np.m0.compare）", () => {
  it("保存后能原样读回，并写入约定 key", () => {
    const storage = memoryStorage();
    const scores: ComparisonScores = { blocks: scoreSet(4), xterm: scoreSet(2) };

    saveScores(scores, storage);

    expect(storage.data.has(SCORE_STORAGE_KEY)).toBe(true);
    expect(loadScores(storage)).toEqual(scores);
  });

  it("JSON 损坏或分值越界时整体丢弃，回到 null（由调用方用默认基线）", () => {
    const storage = memoryStorage();

    storage.setItem(SCORE_STORAGE_KEY, "{not-json");
    expect(loadScores(storage)).toBeNull();

    storage.setItem(
      SCORE_STORAGE_KEY,
      JSON.stringify({
        version: 1,
        scores: { blocks: scoreSet(9), xterm: scoreSet(3) },
      }),
    );
    expect(loadScores(storage)).toBeNull();
    expect(parseStoredScores(null)).toBeNull();
    expect(isComparisonScores({ blocks: scoreSet(3), xterm: scoreSet(3) })).toBe(true);
  });
});

describe("导出 JSON", () => {
  const scores: ComparisonScores = {
    blocks: scoreSet(5),
    xterm: scoreSet(2),
  };
  const resources: ResourceSizeEntry[] = [
    {
      name: "http://127.0.0.1:1424/node_modules/.vite/deps/@xterm_xterm.js",
      encodedBodySize: 1000,
    },
    {
      name: "http://127.0.0.1:1424/src/features/terminal/renderers/blocks/BlocksView.tsx",
      transferSize: 400,
    },
    {
      name: "http://127.0.0.1:1424/src/main.tsx",
      encodedBodySize: 50,
    },
  ];

  it("包含时间戳、权重、双方案总分、领先方与包体积粗算", () => {
    const payload = buildExportPayload({
      scores,
      variant: "both",
      theme: "amber",
      now: new Date("2026-11-08T09:30:00.000Z"),
      resources,
    });

    expect(payload.createdAt).toBe("2026-11-08T09:30:00.000Z");
    expect(payload.schemaVersion).toBe(1);
    expect(payload.weights.map((item) => item.weight)).toEqual([30, 25, 20, 15, 10]);
    expect(payload.totals).toEqual({ blocks: 5, xterm: 2 });
    expect(payload.leader).toBe("blocks");
    expect(payload.variant).toBe("both");
    expect(payload.theme).toBe("amber");
    expect(payload.bundleBytes).toEqual({ blocks: 400, xterm: 1000 });
  });

  it("拿不到资源体积时写 null，不编造数字", () => {
    expect(sumRendererBytes([])).toEqual({ blocks: null, xterm: null });
  });
});
