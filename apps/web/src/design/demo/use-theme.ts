import { useCallback, useState } from "react";

export const THEMES = ["graphite", "amber", "ice"] as const;

export type Theme = (typeof THEMES)[number];

export function isTheme(value: string | null | undefined): value is Theme {
  return value === "graphite" || value === "amber" || value === "ice";
}

/** demo 支持 ?theme=amber 这类直链，便于截图与回归。 */
export function readThemeFromUrl(): Theme {
  const fromUrl = new URLSearchParams(window.location.search).get("theme");
  return isTheme(fromUrl) ? fromUrl : "graphite";
}

export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
}

export function useTheme() {
  const [theme, setThemeState] = useState<Theme>(readThemeFromUrl);

  // 先同步改 DOM 属性，再 setState：这样同一帧内读到的计算值已是新主题。
  const setTheme = useCallback((next: Theme) => {
    applyTheme(next);
    setThemeState(next);
  }, []);

  return { theme, setTheme };
}
