import { getCurrentWindow } from "@tauri-apps/api/window";

export type WindowLabel = "player" | "terminal";

/** 浏览器直接打开（无 Tauri 运行时）时的兜底窗口。 */
export const DEFAULT_WINDOW_LABEL: WindowLabel = "player";

/**
 * 读取当前窗口 label 并映射为视图名。
 * 不在 Tauri 里运行时 `getCurrentWindow()` 会抛错，按浏览器默认值处理。
 */
export function resolveWindowLabel(): WindowLabel {
  try {
    return getCurrentWindow().label === "terminal" ? "terminal" : "player";
  } catch {
    return DEFAULT_WINDOW_LABEL;
  }
}
