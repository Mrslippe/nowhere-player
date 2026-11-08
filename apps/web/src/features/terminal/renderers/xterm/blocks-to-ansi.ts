/**
 * T-0007：把结构化 `Block[]` 降级成 ANSI 文本（ARCH-005 §2.2 / §7）。
 *
 * 纯函数：不碰 DOM、不读全局状态，同一输入永远得到同一输出。
 * 行尾统一以 `\x1b[0m` 复位；`progress` 按契约不加结尾换行，便于后续 `\r` 原地刷新。
 */
import type {
  Block,
  ErrorOutput,
  HintOutput,
  MediaOutput,
  Output,
  ProgressOutput,
  Span,
  TableOutput,
  TextOutput,
  Tone,
} from "../../../../shared/terminal-model";
import { normalizeOutput } from "../../../../shared/terminal-model.guard";

/** ANSI SGR 复位。 */
export const ANSI_RESET = "\u001b[0m";

const ANSI_BOLD = "\u001b[1m";
const ANSI_DIM = "\u001b[2m";
const ANSI_ITALIC = "\u001b[3m";

/** 语义色 → ANSI 256 索引（ARCH-005 §4）。 */
export const SEMANTIC_ANSI_COLOR: Readonly<Record<Tone, number>> = {
  accent: 111,
  danger: 203,
  warn: 179,
  success: 114,
  muted: 244,
};

/** 默认前景色索引（ARCH-005 §4 的 `null`）。 */
export const DEFAULT_ANSI_COLOR = 252;

/** 进度条固定宽度（字符格）。 */
export const PROGRESS_BAR_WIDTH = 20;

const INPUT_PREFIX = "› ";
const ERROR_PREFIX = "✗ ";

function colorIndex(tone: Tone | null): number {
  return tone === null ? DEFAULT_ANSI_COLOR : SEMANTIC_ANSI_COLOR[tone];
}

function colorCode(tone: Tone | null): string {
  return `\u001b[38;5;${colorIndex(tone)}m`;
}

/**
 * 单个码点是否为全角（占 2 格）。
 *
 * 用码点区间近似，覆盖任务卡 T-0007 §11 给定的区间；不引第三方宽度库。
 */
function isWideCodePoint(codePoint: number): boolean {
  return (
    (codePoint >= 0x1100 && codePoint <= 0x115f) ||
    (codePoint >= 0x2e80 && codePoint <= 0xa4cf) ||
    (codePoint >= 0xac00 && codePoint <= 0xd7a3) ||
    (codePoint >= 0xf900 && codePoint <= 0xfaff) ||
    (codePoint >= 0xfe30 && codePoint <= 0xfe4f) ||
    (codePoint >= 0xff00 && codePoint <= 0xff60) ||
    (codePoint >= 0xffe0 && codePoint <= 0xffe6)
  );
}

/** 字符串在终端里的显示宽度：CJK 记 2，其余记 1。 */
export function displayWidth(text: string): number {
  let width = 0;
  for (const character of text) {
    width += isWideCodePoint(character.codePointAt(0) ?? 0) ? 2 : 1;
  }
  return width;
}

/** 按显示宽度右侧补空格；超宽不改写（由调用方决定截断策略）。 */
function padToWidth(text: string, width: number): string {
  const padding = width - displayWidth(text);
  return padding > 0 ? text + " ".repeat(padding) : text;
}

/** span 的 SGR 前缀：字重/字形逐个输出，颜色最后覆盖。 */
function spanStyle(span: Span, tone: Tone | null): string {
  let style = "";
  if (span.bold) {
    style += ANSI_BOLD;
  }
  if (span.dim) {
    style += ANSI_DIM;
  }
  if (span.italic) {
    style += ANSI_ITALIC;
  }
  return style + colorCode(span.color ?? tone);
}

function renderText(output: TextOutput): string {
  const lines: string[] = [];
  let line = "";
  const endLine = (): void => {
    lines.push(line + ANSI_RESET);
    line = "";
  };

  for (const span of output.spans) {
    const style = spanStyle(span, output.tone);
    const pieces = span.text.split("\n");
    pieces.forEach((piece, index) => {
      if (index > 0) {
        endLine();
      }
      if (piece === "") {
        return;
      }
      // 换行后，样式状态可能残留；进入新 span 前先复位，避免样式串行。
      line += (line === "" ? "" : ANSI_RESET) + style + piece;
    });
  }
  endLine();

  return lines.join("\n") + "\n";
}

function renderTable(output: TableOutput): string {
  const columns = output.columns;
  if (columns.length === 0) {
    return "";
  }

  const widths = columns.map((column, columnIndex) => {
    let width = displayWidth(column);
    for (const row of output.rows) {
      width = Math.max(width, displayWidth(row[columnIndex] ?? ""));
    }
    return width;
  });

  const formatRow = (cells: string[]): string =>
    columns
      .map((_, columnIndex) => {
        const cell = cells[columnIndex] ?? "";
        // 末列不补空格，避免每行尾随空白；列宽仍按最大值计算。
        return columnIndex === columns.length - 1 ? cell : padToWidth(cell, widths[columnIndex]);
      })
      .join("  ");

  const lines = [`${ANSI_DIM}${formatRow(columns)}${ANSI_RESET}`];
  for (const row of output.rows) {
    lines.push(`${formatRow(row)}${ANSI_RESET}`);
  }

  return lines.join("\n") + "\n";
}

function renderProgress(output: ProgressOutput): string {
  const total = output.total > 0 ? output.total : 0;
  const ratio = total === 0 ? 0 : Math.min(1, Math.max(0, output.current / total));
  const filled = Math.floor(ratio * PROGRESS_BAR_WIDTH);
  const percent = Math.floor(ratio * 100);
  const bar = `${"█".repeat(filled)}${"░".repeat(PROGRESS_BAR_WIDTH - filled)}`;
  const label = output.label === "" ? "" : `${output.label} `;

  // 不加结尾换行（任务卡 §6.1），后续内容由拼接逻辑补换行。
  return `${label}[${bar}] ${percent}%${ANSI_RESET}`;
}

function renderError(output: ErrorOutput): string {
  const lines = [`${colorCode("danger")}${ERROR_PREFIX}${output.code}${ANSI_RESET}`];
  for (const messageLine of output.message.split("\n")) {
    lines.push(`  ${messageLine}${ANSI_RESET}`);
  }
  if (output.hint !== null) {
    for (const hintLine of output.hint.split("\n")) {
      lines.push(`  ${ANSI_DIM}${hintLine}${ANSI_RESET}`);
    }
  }
  return lines.join("\n") + "\n";
}

function renderHint(output: HintOutput): string {
  const [first = "", ...rest] = output.text.split("\n");
  const lines = [`· ${ANSI_DIM}${first}${ANSI_RESET}`];
  for (const restLine of rest) {
    lines.push(`${ANSI_DIM}${restLine}${ANSI_RESET}`);
  }
  return lines.join("\n") + "\n";
}

function renderMedia(output: MediaOutput): string {
  // 未知 kind 按 ARCH-005 §7.3 降级为 cover。
  const placeholder = output.kind === "thumb" ? "[thumb]" : "[cover]";
  return `${ANSI_DIM}${placeholder}${ANSI_RESET}\n`;
}

function renderOutput(output: Output): string {
  switch (output.type) {
    case "text":
      return renderText(output);
    case "table":
      return renderTable(output);
    case "progress":
      return renderProgress(output);
    case "error":
      return renderError(output);
    case "hint":
      return renderHint(output);
    case "media":
      return renderMedia(output);
    default:
      return "";
  }
}

function renderInputLine(block: Block): string {
  const suffix = block.status === "error" ? " ✗" : "";
  return `${ANSI_BOLD}${INPUT_PREFIX}${block.input}${suffix}${ANSI_RESET}\n`;
}

function renderFooter(footer: string): string {
  return `${ANSI_DIM}${footer}${ANSI_RESET}\n`;
}

/** 拼接片段；若前一段没有以换行收尾（progress），先补一个换行再拼接。 */
function appendChunk(buffer: string, chunk: string): string {
  if (chunk === "") {
    return buffer;
  }
  const nextBuffer = buffer !== "" && !buffer.endsWith("\n") ? `${buffer}\n` : buffer;
  return nextBuffer + chunk;
}

function renderBlock(block: Block): string {
  let blockText = renderInputLine(block);
  for (const output of block.outputs) {
    // 未知 Output.type 在共享 guard 里降级为 text + `[unsupported]`（ARCH-005 §5）。
    blockText = appendChunk(blockText, renderOutput(normalizeOutput(output)));
  }
  if (block.footer !== null) {
    blockText = appendChunk(blockText, renderFooter(block.footer));
  }
  return blockText;
}

/**
 * 把终端输出模型降级成 ANSI 文本（xterm 渲染器用）。
 *
 * - 每个块以输入行 `› ...` 开头，随后按顺序渲染 outputs，最后是 footer（若有）。
 * - 除 `progress` 外每行都以 `\x1b[0m` 收尾；`progress` 自身不带换行。
 */
export function blocksToAnsi(blocks: Block[]): string {
  let buffer = "";
  for (const block of blocks) {
    buffer = appendChunk(buffer, renderBlock(block));
  }
  return buffer;
}
