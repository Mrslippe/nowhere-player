import { getCurrentWindow } from "@tauri-apps/api/window";

export type WindowLabel = "player" | "terminal";

/** 浏览器直接打开（无 Tauri 运行时）时的兜底窗口。 */
export const DEFAULT_WINDOW_LABEL: WindowLabel = "player";

/**
 * 读取当前窗口 label 并映射为视图名。
 *
 * 优先级：
 * 1. 查询参数 `?window=player|terminal`（浏览器预览用，Tauri 运行时也可强制指定）
 * 2. Tauri 窗口 label
 * 3. 浏览器兜底 `player`
 */
export function resolveWindowLabel(): WindowLabel {
  if (typeof window !== "undefined") {
    const fromQuery = new URLSearchParams(window.location.search).get("window");
    if (fromQuery === "player" || fromQuery === "terminal") {
      return fromQuery;
    }
  }

  try {
    return getCurrentWindow().label === "terminal" ? "terminal" : "player";
  } catch {
    return DEFAULT_WINDOW_LABEL;
  }
}
