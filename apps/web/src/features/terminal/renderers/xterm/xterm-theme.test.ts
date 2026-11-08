import { describe, expect, it } from "vitest";
import { DEFAULT_ANSI_COLOR, SEMANTIC_ANSI_COLOR } from "./blocks-to-ansi";
import { XTERM_OPTIONS, XTERM_THEME } from "./xterm-theme";

/** `extendedAnsi[0]` 对应 ANSI 16。 */
const colorAt = (index: number): string | undefined => {
  const palette = XTERM_THEME.extendedAnsi;
  if (palette === undefined) {
    throw new Error("XTERM_THEME.extendedAnsi 缺失，256 色索引无法落到 graphite 色值");
  }
  return palette[index - 16];
};

describe("xterm 主题", () => {
  it("语义色 → ANSI 256 索引符合 ARCH-005 §4", () => {
    expect(SEMANTIC_ANSI_COLOR).toEqual({
      accent: 111,
      danger: 203,
      warn: 179,
      success: 114,
      muted: 244,
    });
    expect(DEFAULT_ANSI_COLOR).toBe(252);
  });

  it("extendedAnsi 把 6 个索引映射到 graphite 色值", () => {
    expect(XTERM_THEME.extendedAnsi).toHaveLength(240);
    expect(colorAt(SEMANTIC_ANSI_COLOR.accent)).toBe("#7c9ef5");
    expect(colorAt(SEMANTIC_ANSI_COLOR.danger)).toBe("#f2707a");
    expect(colorAt(SEMANTIC_ANSI_COLOR.warn)).toBe("#e8b45c");
    expect(colorAt(SEMANTIC_ANSI_COLOR.success)).toBe("#7fc79a");
    expect(colorAt(SEMANTIC_ANSI_COLOR.muted)).toBe("#8b9096");
    expect(colorAt(DEFAULT_ANSI_COLOR)).toBe("#e8eaed");
  });

  it("背景 / 前景 / 光标取 graphite", () => {
    expect(XTERM_THEME.background).toBe("#0b0c0e");
    expect(XTERM_THEME.foreground).toBe("#e8eaed");
    expect(XTERM_THEME.cursor).toBe("#7c9ef5");
  });

  it("终端选项：等宽 13px、无输入、LF 当 CRLF", () => {
    expect(XTERM_OPTIONS.fontSize).toBe(13);
    expect(XTERM_OPTIONS.lineHeight).toBe(1.6);
    expect(XTERM_OPTIONS.disableStdin).toBe(true);
    // blocksToAnsi 只写 \n，必须由 xterm 补回车，否则输出会阶梯排列。
    expect(XTERM_OPTIONS.convertEol).toBe(true);
    expect(XTERM_OPTIONS.theme).toBe(XTERM_THEME);
  });
});
