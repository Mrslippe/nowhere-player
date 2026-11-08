import type { Block, BlockStatus, Output, Span, SpanColor, Tone } from "./terminal-model";

/** 未知 `Output.type` 的降级文本，与 Rust 侧保持一致（ARCH-005 §5）。 */
export const UNSUPPORTED_OUTPUT_TEXT = "[unsupported]";

const BLOCK_STATUSES: readonly BlockStatus[] = [
  "pending",
  "running",
  "done",
  "error",
  "cancelled",
];

const TONES: readonly Tone[] = ["accent", "danger", "warn", "success", "muted"];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isString(value: unknown): value is string {
  return typeof value === "string";
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every(isString);
}

function isBlockStatus(value: unknown): value is BlockStatus {
  return typeof value === "string" && (BLOCK_STATUSES as readonly string[]).includes(value);
}

function isTone(value: unknown): value is Tone {
  return typeof value === "string" && (TONES as readonly string[]).includes(value);
}

function isSpanColor(value: unknown): value is SpanColor {
  return typeof value === "string" && (TONES as readonly string[]).includes(value);
}

function isSpan(value: unknown): value is Span {
  return (
    isRecord(value) &&
    isString(value.text) &&
    typeof value.bold === "boolean" &&
    typeof value.dim === "boolean" &&
    typeof value.italic === "boolean" &&
    (value.color === null || isSpanColor(value.color))
  );
}

/** 判断一个未知值是否符合六种 Output 之一的序列化形状。 */
export function isOutput(value: unknown): value is Output {
  if (!isRecord(value) || !isString(value.type)) {
    return false;
  }

  switch (value.type) {
    case "text":
      return (
        Array.isArray(value.spans) &&
        value.spans.every(isSpan) &&
        (value.tone === null || isTone(value.tone))
      );
    case "table":
      return (
        isStringArray(value.columns) &&
        Array.isArray(value.rows) &&
        value.rows.every((row) => isStringArray(row)) &&
        typeof value.selectable === "boolean"
      );
    case "progress":
      return (
        isString(value.label) &&
        isFiniteNumber(value.current) &&
        isFiniteNumber(value.total) &&
        isString(value.unit)
      );
    case "error":
      return (
        isString(value.code) &&
        isString(value.message) &&
        (value.hint === null || isString(value.hint)) &&
        (value.actions === null || isStringArray(value.actions))
      );
    case "hint":
      return isString(value.text) && (value.suggestion === null || isString(value.suggestion));
    case "media":
      return (
        (value.kind === "cover" || value.kind === "thumb") &&
        isString(value.ref) &&
        isFiniteNumber(value.size)
      );
    default:
      return false;
  }
}

/**
 * 把未知值归一化成可渲染的 Output。
 *
 * 未知 `type`（以及形状不完整的未知值）降级为 `text` + `[unsupported]`，
 * 与 Rust 侧反序列化行为一致（ARCH-005 §5）。
 */
export function normalizeOutput(value: unknown): Output {
  if (isOutput(value)) {
    return value;
  }

  return {
    type: "text",
    tone: null,
    spans: [
      {
        text: UNSUPPORTED_OUTPUT_TEXT,
        bold: false,
        dim: false,
        italic: false,
        color: null,
      },
    ],
  };
}

/** 判断一个未知值是否符合 Block 的序列化形状。 */
export function isBlock(value: unknown): value is Block {
  return (
    isRecord(value) &&
    isFiniteNumber(value.schemaVersion) &&
    isString(value.id) &&
    isString(value.input) &&
    isFiniteNumber(value.startedAt) &&
    (value.durationMs === null || isFiniteNumber(value.durationMs)) &&
    isBlockStatus(value.status) &&
    Array.isArray(value.outputs) &&
    value.outputs.every(isOutput) &&
    (value.footer === null || isString(value.footer))
  );
}
