/**
 * T-0008 §6.5：评分导出。
 *
 * 导出内容 = 评分 + 时间戳 + 两套渲染器的包体积（粗算）。
 * 包体积来自 `performance.getEntriesByType('resource')`：dev 下是未压缩模块；
 * 拿不到或为 0 时写 `null`，不猜数字。
 */
import {
  compareScores,
  SCORE_DIMENSIONS,
  type ComparisonScores,
  type ScoreSide,
} from "./scoring";
import type { LabTheme } from "./theme";
import type { LabVariant } from "./variant";

export interface ResourceSizeEntry {
  name: string;
  encodedBodySize?: number;
  decodedBodySize?: number;
  transferSize?: number;
}

export interface BundleBytes {
  blocks: number | null;
  xterm: number | null;
}

export interface CompareExportPayload {
  schemaVersion: 1;
  kind: "np.m0.compare";
  createdAt: string;
  variant: LabVariant;
  theme: LabTheme;
  weights: Array<{ id: string; label: string; weight: number }>;
  scores: ComparisonScores;
  totals: Record<ScoreSide, number>;
  leader: ScoreSide | "tie";
  margin: number;
  bundleBytes: BundleBytes;
  bundleNote: string;
}

function entryBytes(entry: ResourceSizeEntry): number {
  return Math.max(
    entry.encodedBodySize ?? 0,
    entry.decodedBodySize ?? 0,
    entry.transferSize ?? 0,
  );
}

/** 按资源 URL 粗分两套渲染器：xterm 依赖 vs 结构化渲染器目录。 */
export function sumRendererBytes(
  entries: readonly ResourceSizeEntry[],
): BundleBytes {
  let blocks = 0;
  let xterm = 0;

  for (const entry of entries) {
    const url = entry.name;
    const bytes = entryBytes(entry);

    if (url.includes("@xterm") || url.includes("/xterm/")) {
      xterm += bytes;
    } else if (url.includes("/renderers/blocks/") || url.includes("/blocks/")) {
      blocks += bytes;
    }
  }

  return {
    blocks: blocks > 0 ? blocks : null,
    xterm: xterm > 0 ? xterm : null,
  };
}

export function collectResourceEntries(): ResourceSizeEntry[] {
  if (typeof performance === "undefined" || typeof performance.getEntriesByType !== "function") {
    return [];
  }

  try {
    return performance.getEntriesByType("resource") as ResourceSizeEntry[];
  } catch {
    return [];
  }
}

export interface BuildExportOptions {
  scores: ComparisonScores;
  variant: LabVariant;
  theme: LabTheme;
  now: Date;
  resources: readonly ResourceSizeEntry[];
}

export function buildExportPayload({
  scores,
  variant,
  theme,
  now,
  resources,
}: BuildExportOptions): CompareExportPayload {
  const comparison = compareScores(scores);

  return {
    schemaVersion: 1,
    kind: "np.m0.compare",
    createdAt: now.toISOString(),
    variant,
    theme,
    weights: SCORE_DIMENSIONS.map((dimension) => ({
      id: dimension.id,
      label: dimension.label,
      weight: dimension.weight,
    })),
    scores: {
      blocks: { ...scores.blocks },
      xterm: { ...scores.xterm },
    },
    totals: comparison.totals,
    leader: comparison.leader,
    margin: comparison.margin,
    bundleBytes: sumRendererBytes(resources),
    bundleNote: "performance resource 粗算；dev 下为未压缩模块大小，生产包需重新量。",
  };
}

/** 写剪贴板；Clipboard API 不可用（非安全上下文 / 权限拒绝）时退回 execCommand。 */
export async function copyToClipboard(text: string): Promise<boolean> {
  if (typeof navigator !== "undefined" && navigator.clipboard !== undefined) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // 继续走兜底路径。
    }
  }

  if (typeof document === "undefined") {
    return false;
  }

  try {
    const textarea = document.createElement("textarea");
    textarea.value = text;
    textarea.setAttribute("readonly", "");
    textarea.style.position = "fixed";
    textarea.style.top = "0";
    textarea.style.opacity = "0";
    document.body.appendChild(textarea);
    textarea.select();
    const copied = document.execCommand("copy");
    textarea.remove();
    return copied;
  } catch {
    return false;
  }
}
