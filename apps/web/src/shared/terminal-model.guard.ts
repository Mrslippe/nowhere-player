import type {
  Block,
  BlockAction,
  BlockStatus,
  Output,
  Span,
  SpanColor,
  Tone,
} from "./terminal-model";

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

/**
 * 读取块级 actions（ARCH-005 §8）。
 *
 * 非法形状（非数组 / 任一元素缺 label|input / 空字符串）时**整组降级为空**
 * （返回 `undefined`），但 Block 本体保留；旧块缺字段也返回 `undefined`。
 */
export function readBlockActions(value: unknown): BlockAction[] | undefined {
  if (value === undefined || value === null) {
    return undefined;
  }
  if (!Array.isArray(value)) {
    return undefined;
  }

  const actions: BlockAction[] = [];
  for (const item of value) {
    if (!isRecord(item) || !isString(item.label) || !isString(item.input)) {
      return undefined;
    }
    const label = item.label.trim();
    const input = item.input.trim();
    if (label.length === 0 || input.length === 0) {
      return undefined;
    }
    actions.push({ label, input });
  }

  return actions;
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

  const media = normalizeMedia(value);
  if (media) {
    return media;
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

/**
 * 未知 `media.kind` 按契约降级为 `cover`，而不是整条丢弃（ARCH-005 §7.3）。
 * 仅在 `ref` / `size` 合法时生效，否则交给未知类型兜底。
 */
function normalizeMedia(value: unknown): Output | null {
  if (!isRecord(value) || value.type !== "media") {
    return null;
  }
  if (!isString(value.ref) || !isFiniteNumber(value.size)) {
    return null;
  }

  return {
    type: "media",
    kind: value.kind === "thumb" ? "thumb" : "cover",
    ref: value.ref,
    size: value.size,
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
