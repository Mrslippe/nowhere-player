/**
 * demo 页专用：WCAG 对比度计算（正文目标 ≥ 4.5:1）。
 * 只处理不透明前景色，带 alpha 的 token 不参与对比度断言。
 */
import { resolvedColor } from "./probe";

export type Rgba = { r: number; g: number; b: number; a: number };

export function parseColor(value: string): Rgba | null {
  const match = /rgba?\(([^)]+)\)/i.exec(value);
  if (!match) {
    return null;
  }

  const parts = match[1]
    .split(/[\s,/]+/)
    .filter(Boolean)
    .map((part) => Number(part));

  if (parts.length < 3 || parts.some((part) => Number.isNaN(part))) {
    return null;
  }

  return { r: parts[0], g: parts[1], b: parts[2], a: parts[3] ?? 1 };
}

function linearize(channel: number): number {
  const value = channel / 255;
  return value <= 0.03928
    ? value / 12.92
    : ((value + 0.055) / 1.055) ** 2.4;
}

export function relativeLuminance(color: Rgba): number {
  return (
    0.2126 * linearize(color.r) +
    0.7152 * linearize(color.g) +
    0.0722 * linearize(color.b)
  );
}

export function contrastRatio(foreground: Rgba, background: Rgba): number {
  const fg = relativeLuminance(foreground);
  const bg = relativeLuminance(background);
  const lighter = Math.max(fg, bg);
  const darker = Math.min(fg, bg);
  return (lighter + 0.05) / (darker + 0.05);
}

/** 返回两个 token 的对比度；解析失败返回 null。 */
export function contrastOfTokens(
  foregroundToken: string,
  backgroundToken: string,
): number | null {
  const foreground = parseColor(resolvedColor(foregroundToken));
  const background = parseColor(resolvedColor(backgroundToken));

  if (!foreground || !background) {
    return null;
  }

  return contrastRatio(foreground, background);
}
