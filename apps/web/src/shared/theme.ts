/**
 * 主题持久化（DES-001 §3）。
 *
 * 存储键固定为 `np.theme`，值为 `graphite` / `amber` / `ice`。
 * `localStorage` 在隐私模式下可能抛错，读写全部包 try/catch；失败时静默回落默认主题。
 */

export const THEME_NAMES = ["graphite", "amber", "ice"] as const;

export type ThemeName = (typeof THEME_NAMES)[number];

export const DEFAULT_THEME: ThemeName = "graphite";

/** 与 `data-theme` 的取值一致，便于直接对照。 */
export const THEME_STORAGE_KEY = "np.theme";

export function isThemeName(value: unknown): value is ThemeName {
  return (
    typeof value === "string" &&
    (THEME_NAMES as readonly string[]).includes(value)
  );
}

/** 读回上次主题；缺失、非法或存储不可用时回落 graphite。 */
export function loadTheme(): ThemeName {
  try {
    const stored = globalThis.localStorage?.getItem(THEME_STORAGE_KEY);
    return isThemeName(stored) ? stored : DEFAULT_THEME;
  } catch {
    return DEFAULT_THEME;
  }
}

/** 写入主题；存储不可用（隐私模式、配额满）时不抛出。 */
export function saveTheme(name: ThemeName): void {
  try {
    globalThis.localStorage?.setItem(THEME_STORAGE_KEY, name);
  } catch {
    // 持久化失败不影响本次会话，下一次尝试仍会重新写入。
  }
}

function themeFromQuery(): ThemeName | null {
  if (typeof window === "undefined") {
    return null;
  }

  try {
    const value = new URLSearchParams(window.location.search).get("theme");
    return isThemeName(value) ? value : null;
  } catch {
    return null;
  }
}

/**
 * 把读回的主题塞进现有的 `?theme=` 通道。
 *
 * 终端实验室的切换逻辑（T-0008）以 URL 为输入源；本卡不改 `features/terminal/**`，
 * 所以启动时若 URL 没带 theme，就把存储值补进去，让既有逻辑自然消费。
 */
function seedThemeQuery(theme: ThemeName): void {
  if (typeof window === "undefined" || themeFromQuery() !== null) {
    return;
  }

  try {
    const url = new URL(window.location.href);
    url.searchParams.set("theme", theme);
    window.history.replaceState(null, "", url);
  } catch {
    // 改不了 URL 就算了，DOM 上的 data-theme 已经落到读回值。
  }
}

function currentDocumentTheme(): ThemeName | null {
  if (typeof document === "undefined") {
    return null;
  }

  const value = document.documentElement.dataset.theme;
  return isThemeName(value) ? value : null;
}

let installed = false;

/**
 * 启动时读回主题，并监听 `<html data-theme>` 的变化写回 localStorage。
 *
 * 入口挂在 `design/index.ts`（设计资源唯一入口，两个窗口都会加载）：
 * 本卡授权路径不含 `app/**` 与 `features/terminal/**`，暂不能放到 App/ThemeProvider。
 * 返回清理函数，便于将来迁移到 Provider 时解绑。
 */
export function installThemePersistence(): () => void {
  if (installed || typeof document === "undefined") {
    return () => {};
  }

  installed = true;

  const root = document.documentElement;
  const initial = themeFromQuery() ?? loadTheme();

  // 先落 DOM，避免首帧闪一下默认主题；URL 显式指定 theme 时以 URL 为准。
  if (!root.dataset.theme) {
    root.dataset.theme = initial;
  }

  seedThemeQuery(initial);

  const observer = new MutationObserver(() => {
    const next = currentDocumentTheme();

    if (next) {
      saveTheme(next);
    }
  });

  observer.observe(root, { attributes: true, attributeFilter: ["data-theme"] });

  return () => {
    observer.disconnect();
    installed = false;
  };
}
