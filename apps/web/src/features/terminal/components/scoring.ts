/**
 * T-0008 §6.5：五维评分与加权总分。
 *
 * 权重必须与 `docs/30-计划/M0-任务清单.md` §3 完全一致（30/25/20/15/10），
 * 不得在 UI 里另写一份数字。每维 1–5 分，总分 = Σ(得分 × 权重) ÷ 100。
 */

export const SCORE_SIDES = ["blocks", "xterm"] as const;

export type ScoreSide = (typeof SCORE_SIDES)[number];

export const SCORE_DIMENSIONS = [
  { id: "visual", label: "视觉完成度", weight: 30 },
  { id: "interaction", label: "交互体验", weight: 25 },
  { id: "ime", label: "输入法（IME）", weight: 20 },
  { id: "performance", label: "性能", weight: 15 },
  { id: "extensibility", label: "后续扩展成本", weight: 10 },
] as const;

export type DimensionId = (typeof SCORE_DIMENSIONS)[number]["id"];

export type ScoreSet = Record<DimensionId, number>;

export type ComparisonScores = Record<ScoreSide, ScoreSet>;

export const SIDE_LABELS: Readonly<Record<ScoreSide, string>> = {
  blocks: "A · 结构化",
  xterm: "B · xterm",
};

export const SIDE_KEYS: Readonly<Record<ScoreSide, string>> = {
  blocks: "A",
  xterm: "B",
};

export const SCORE_MIN = 1;
export const SCORE_MAX = 5;

export const SCORE_STORAGE_KEY = "np.m0.compare";

/**
 * 自评基线（2026-11-08）：用于让面板一打开就有可导出的初始分。
 * 这是实现者的主观自评，不是测量结果；改分后本基线会被 localStorage 覆盖。
 */
export const DEFAULT_SCORES: ComparisonScores = {
  blocks: {
    visual: 4,
    interaction: 4,
    ime: 5,
    performance: 4,
    extensibility: 5,
  },
  xterm: {
    visual: 3,
    interaction: 4,
    ime: 5,
    performance: 2,
    extensibility: 2,
  },
};

export function clampScore(value: number): number {
  if (!Number.isFinite(value)) {
    return SCORE_MIN;
  }

  return Math.min(SCORE_MAX, Math.max(SCORE_MIN, Math.round(value)));
}

/** 加权总分（5 分制，保留两位小数）。 */
export function weightedTotal(scores: ScoreSet): number {
  const sum = SCORE_DIMENSIONS.reduce(
    (total, dimension) => total + clampScore(scores[dimension.id]) * dimension.weight,
    0,
  );

  return Math.round((sum / 100) * 100) / 100;
}

export function formatScore(value: number): string {
  return value.toFixed(2);
}

export interface ScoreComparison {
  totals: Record<ScoreSide, number>;
  leader: ScoreSide | "tie";
  margin: number;
}

export function compareScores(scores: ComparisonScores): ScoreComparison {
  const totals: Record<ScoreSide, number> = {
    blocks: weightedTotal(scores.blocks),
    xterm: weightedTotal(scores.xterm),
  };
  const difference = Math.round((totals.blocks - totals.xterm) * 100) / 100;

  return {
    totals,
    leader: difference > 0 ? "blocks" : difference < 0 ? "xterm" : "tie",
    margin: Math.abs(difference),
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isScoreSet(value: unknown): value is ScoreSet {
  if (!isRecord(value)) {
    return false;
  }

  return SCORE_DIMENSIONS.every((dimension) => {
    const score = value[dimension.id];

    return (
      typeof score === "number" &&
      Number.isInteger(score) &&
      score >= SCORE_MIN &&
      score <= SCORE_MAX
    );
  });
}

export function isComparisonScores(value: unknown): value is ComparisonScores {
  return isRecord(value) && isScoreSet(value.blocks) && isScoreSet(value.xterm);
}

interface StoredScores {
  version: number;
  scores: ComparisonScores;
  updatedAt: string;
}

/** 解析 localStorage 里的 JSON；任何字段不合法都整体丢弃，回到默认基线。 */
export function parseStoredScores(raw: string | null): ComparisonScores | null {
  if (raw === null) {
    return null;
  }

  try {
    const parsed: unknown = JSON.parse(raw);

    if (!isRecord(parsed)) {
      return null;
    }

    return isComparisonScores(parsed.scores) ? parsed.scores : null;
  } catch {
    return null;
  }
}

type StorageLike = Pick<Storage, "getItem" | "setItem">;

function defaultStorage(): StorageLike | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

export function loadScores(storage?: StorageLike | null): ComparisonScores | null {
  const target = storage === undefined ? defaultStorage() : storage;

  if (target === null) {
    return null;
  }

  try {
    return parseStoredScores(target.getItem(SCORE_STORAGE_KEY));
  } catch {
    return null;
  }
}

export function saveScores(scores: ComparisonScores, storage?: StorageLike | null): void {
  const target = storage === undefined ? defaultStorage() : storage;

  if (target === null) {
    return;
  }

  const payload: StoredScores = {
    version: 1,
    scores,
    updatedAt: new Date().toISOString(),
  };

  try {
    target.setItem(SCORE_STORAGE_KEY, JSON.stringify(payload));
  } catch {
    // 隐私模式 / 配额满时静默降级：评分仍可用，只是不持久化。
  }
}
