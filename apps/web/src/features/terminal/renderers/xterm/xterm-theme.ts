/**
 * xterm 主题与终端选项（T-0007 §6.2 / T-0029 §6.5）。
 *
 * xterm 的 canvas 渲染读不到 CSS 变量，主题只能在 JS 里显式给色值；
 * 因此本文件是组件侧唯一允许出现色值字面量的位置，且字面量只作为
 * **CSS 变量读不到时的兜底**（graphite）。正常运行时由 {@link readXtermTheme}
 * 从 `getComputedStyle(document.documentElement)` 取当前主题的色值，
 * `phosphor` 等主题切换后 xterm 也会跟着变色（T-0029 修掉 T-0028 的债）。
 *
 * ANSI 256 索引与 `blocksToAnsi` 共用同一份语义色映射，避免渲染器与主题漂移。
 */
import type { ITheme, ITerminalOptions } from "@xterm/xterm";
import { DEFAULT_ANSI_COLOR, SEMANTIC_ANSI_COLOR } from "./blocks-to-ansi";

/** 兜底调色板：DES-001 §3 · graphite。 */
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

/** 生成 xterm 主题所需的色值集合。 */
export type XtermColors = {
  /** 画布背景。 */
  bg: string;
  /** 抬升背景（ANSI black）。 */
  bgElev: string;
  /** 正文前景。 */
  fg: string;
  /** 次要前景（ANSI brightBlack）。 */
  fgMuted: string;
  /** 强调色（光标 / ANSI blue）。 */
  accent: string;
  /** 错误色（ANSI red）。 */
  danger: string;
  /** 警告色（ANSI yellow）。 */
  warn: string;
  /** 成功色（ANSI green）。 */
  success: string;
};

/** `ITheme.extendedAnsi[0]` 对应 ANSI 16。 */
const EXTENDED_ANSI_OFFSET = 16;

function buildExtendedAnsi(colors: XtermColors): string[] {
  const palette = new Array<string>(256 - EXTENDED_ANSI_OFFSET);
  palette[SEMANTIC_ANSI_COLOR.accent - EXTENDED_ANSI_OFFSET] = colors.accent;
  palette[SEMANTIC_ANSI_COLOR.danger - EXTENDED_ANSI_OFFSET] = colors.danger;
  palette[SEMANTIC_ANSI_COLOR.warn - EXTENDED_ANSI_OFFSET] = colors.warn;
  palette[SEMANTIC_ANSI_COLOR.success - EXTENDED_ANSI_OFFSET] = colors.success;
  palette[SEMANTIC_ANSI_COLOR.muted - EXTENDED_ANSI_OFFSET] = colors.fgMuted;
  palette[DEFAULT_ANSI_COLOR - EXTENDED_ANSI_OFFSET] = colors.fg;
  return palette;
}

/**
 * 用一组色值生成 `ITheme`。
 *
 * 让 `\\x1b[38;5;111m` 等索引真正落到当前主题的色值上。
 * 基础 16 色也按语义对齐，保证将来出现 31–37 的文本仍然协调。
 */
export function buildXtermTheme(colors: XtermColors): ITheme {
  return {
    background: colors.bg,
    foreground: colors.fg,
    cursor: colors.accent,
    cursorAccent: colors.bg,
    black: colors.bgElev,
    brightBlack: colors.fgMuted,
    red: colors.danger,
    brightRed: colors.danger,
    green: colors.success,
    brightGreen: colors.success,
    yellow: colors.warn,
    brightYellow: colors.warn,
    blue: colors.accent,
    brightBlue: colors.accent,
    white: colors.fg,
    brightWhite: colors.fg,
    extendedAnsi: buildExtendedAnsi(colors),
  };
}

/** 读 CSS 自定义属性：`name` 形如 `--bg`。 */
export type CssColorReader = (name: string) => string;

/** 取一个 CSS 变量的值；空串 / 纯空白时回落到兜底色值。 */
export function readCssColor(
  reader: CssColorReader,
  name: string,
  fallback: string,
): string {
  const value = reader(name).trim();
  return value === "" ? fallback : value;
}

/**
 * 默认读取器：`getComputedStyle(document.documentElement)`。
 *
 * 非浏览器环境（SSR / 单测）没有计算样式，返回全空串 → 全部回落到 graphite。
 */
export function computedColorReader(): CssColorReader {
  if (typeof document === "undefined" || typeof getComputedStyle !== "function") {
    return () => "";
  }

  const style = getComputedStyle(document.documentElement);
  return (name) => style.getPropertyValue(name);
}

/**
 * 按当前主题（CSS 变量）生成 xterm 主题（T-0029 §6.5）。
 *
 * 读 `--bg` / `--bg-elev` / `--fg` / `--fg-muted` / `--accent` / `--danger` /
 * `--warn` / `--success`；任一项读不到时单独回落 graphite 对应色。
 */
export function readXtermTheme(
  reader: CssColorReader = computedColorReader(),
): ITheme {
  return buildXtermTheme({
    bg: readCssColor(reader, "--bg", GRAPHITE.bg),
    bgElev: readCssColor(reader, "--bg-elev", GRAPHITE.bgElev),
    fg: readCssColor(reader, "--fg", GRAPHITE.fg),
    fgMuted: readCssColor(reader, "--fg-muted", GRAPHITE.fgMuted),
    accent: readCssColor(reader, "--accent", GRAPHITE.accent),
    danger: readCssColor(reader, "--danger", GRAPHITE.danger),
    warn: readCssColor(reader, "--warn", GRAPHITE.warn),
    success: readCssColor(reader, "--success", GRAPHITE.success),
  });
}

/** graphite 的静态主题（兼容旧导出；运行时请用 {@link readXtermTheme}）。 */
export const XTERM_THEME: ITheme = buildXtermTheme(GRAPHITE);

/**
 * 终端选项：等宽 13px / 行高 1.6（DES-001 §5 配置窗约定）。
 *
 * `convertEol: true` 是因为 `blocksToAnsi` 只产出 `\\n`（LF），
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

/**
 * 把主题换进终端选项。
 *
 * xterm 不支持运行时换主题，切换时只能 dispose 后重建（见 `XtermSurface`）。
 */
export function xtermOptionsFor(theme: ITheme): ITerminalOptions {
  return { ...XTERM_OPTIONS, theme };
}
