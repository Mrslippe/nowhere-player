/**
 * xterm 主题与终端选项（T-0007 §6.2）。
 *
 * xterm 的 canvas 渲染读不到 CSS 变量，主题只能在 JS 里显式给色值；
 * 因此本文件是组件侧唯一允许出现色值字面量的位置，取值全部来自 DES-001 §3 的 graphite 主题。
 *
 * ANSI 256 索引与 `blocksToAnsi` 共用同一份语义色映射，避免渲染器与主题漂移。
 */
import type { ITheme, ITerminalOptions } from "@xterm/xterm";
import { DEFAULT_ANSI_COLOR, SEMANTIC_ANSI_COLOR } from "./blocks-to-ansi";

/** DES-001 §3 · graphite。 */
const GRAPHITE = {
  bg: "#0b0c0e",
  bgElev: "#141619",
  fg: "#e8eaed",
  fgMuted: "#8b9096",
  accent: "#7c9ef5",
  danger: "#f2707a",
  warn: "#e8b45c",
  success: "#7fc79a",
} as const;

/** `ITheme.extendedAnsi[0]` 对应 ANSI 16。 */
const EXTENDED_ANSI_OFFSET = 16;

function buildExtendedAnsi(): string[] {
  const palette = new Array<string>(256 - EXTENDED_ANSI_OFFSET);
  palette[SEMANTIC_ANSI_COLOR.accent - EXTENDED_ANSI_OFFSET] = GRAPHITE.accent;
  palette[SEMANTIC_ANSI_COLOR.danger - EXTENDED_ANSI_OFFSET] = GRAPHITE.danger;
  palette[SEMANTIC_ANSI_COLOR.warn - EXTENDED_ANSI_OFFSET] = GRAPHITE.warn;
  palette[SEMANTIC_ANSI_COLOR.success - EXTENDED_ANSI_OFFSET] = GRAPHITE.success;
  palette[SEMANTIC_ANSI_COLOR.muted - EXTENDED_ANSI_OFFSET] = GRAPHITE.fgMuted;
  palette[DEFAULT_ANSI_COLOR - EXTENDED_ANSI_OFFSET] = GRAPHITE.fg;
  return palette;
}

/**
 * 让 `\x1b[38;5;111m` 等索引真正落到 graphite 色值上。
 * 基础 16 色也按语义对齐，保证将来出现 31–37 的文本仍然协调。
 */
export const XTERM_THEME: ITheme = {
  background: GRAPHITE.bg,
  foreground: GRAPHITE.fg,
  cursor: GRAPHITE.accent,
  cursorAccent: GRAPHITE.bg,
  black: GRAPHITE.bgElev,
  brightBlack: GRAPHITE.fgMuted,
  red: GRAPHITE.danger,
  brightRed: GRAPHITE.danger,
  green: GRAPHITE.success,
  brightGreen: GRAPHITE.success,
  yellow: GRAPHITE.warn,
  brightYellow: GRAPHITE.warn,
  blue: GRAPHITE.accent,
  brightBlue: GRAPHITE.accent,
  white: GRAPHITE.fg,
  brightWhite: GRAPHITE.fg,
  extendedAnsi: buildExtendedAnsi(),
};

/**
 * 终端选项：等宽 13px / 行高 1.6（DES-001 §5 配置窗约定）。
 *
 * `convertEol: true` 是因为 `blocksToAnsi` 只产出 `\n`（LF），
 * 而原生终端语义里 LF 不回车；由 xterm 负责把它当 CRLF 处理。
 */
export const XTERM_OPTIONS: ITerminalOptions = {
  convertEol: true,
  cursorBlink: false,
  disableStdin: true,
  fontFamily: '"JetBrains Mono", "SF Mono", "Cascadia Code", monospace',
  fontSize: 13,
  lineHeight: 1.6,
  scrollback: 2000,
  theme: XTERM_THEME,
};
