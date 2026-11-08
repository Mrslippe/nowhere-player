import { useEffect, useState } from "react";
import { contrastOfTokens } from "./contrast";
import { computedFontFamily, tokenValue } from "./probe";
import { ALL_TOKENS } from "./token-names";
import type { Theme } from "./use-theme";

export type ContrastRow = {
  label: string;
  foreground: string;
  background: string;
  ratio: number | null;
};

export type DesignSnapshot = {
  tokens: Record<string, string>;
  fonts: { ui: string; mono: string };
  contrast: ContrastRow[];
  motion: {
    reduced: boolean;
    spinnerAnimation: string;
    buttonTransition: string;
  };
};

const CONTRAST_PAIRS = [
  { label: "正文", foreground: "--fg", background: "--bg" },
  { label: "次要文本", foreground: "--fg-muted", background: "--bg" },
  { label: "卡片次要文本", foreground: "--fg-muted", background: "--bg-elev" },
  { label: "主按钮文字", foreground: "--accent-fg", background: "--accent" },
] as const;

/** 在浏览器里把 token 解析成当前主题的实际值。 */
function readSnapshot(): DesignSnapshot {
  const tokens: Record<string, string> = Object.fromEntries(
    ALL_TOKENS.map((token) => [token, tokenValue(token)]),
  );

  const contrast = CONTRAST_PAIRS.map((pair) => ({
    ...pair,
    ratio: contrastOfTokens(pair.foreground, pair.background),
  }));

  // 组件已在 DOM 中：直接读它们的计算样式，验证 reduced-motion 降级是否生效。
  const spinner = document.querySelector<SVGElement>(".np-spinner");
  const button = document.querySelector<HTMLButtonElement>(".np-btn");

  return {
    tokens,
    fonts: {
      ui: computedFontFamily("var(--font-ui)"),
      mono: computedFontFamily("var(--font-mono)"),
    },
    contrast,
    motion: {
      reduced: window.matchMedia("(prefers-reduced-motion: reduce)").matches,
      spinnerAnimation: spinner
        ? getComputedStyle(spinner).animationName
        : "n/a",
      buttonTransition: button
        ? getComputedStyle(button).transitionDuration
        : "n/a",
    },
  };
}

/**
 * 首帧先渲染占位，挂载/切主题后再读计算值。
 * 切主题时 useTheme 已同步写好 data-theme，因此 effect 读到的一定是新主题。
 */
export function useDesignSnapshot(theme: Theme): DesignSnapshot | null {
  const [snapshot, setSnapshot] = useState<DesignSnapshot | null>(null);

  useEffect(() => {
    setSnapshot(readSnapshot());
  }, [theme]);

  return snapshot;
}
