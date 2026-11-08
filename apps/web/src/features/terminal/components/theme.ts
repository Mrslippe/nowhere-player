/**
 * 实验室主题切换（DES-001 §3）。
 *
 * 只做三件事：读 `?theme=`、写 `<html data-theme>`、暴露 Theme 类型。
 * 色值仍全部在 `design/themes.css` 里，这里不出现任何色值。
 */

export const LAB_THEMES = ["graphite", "amber", "ice"] as const;

export type LabTheme = (typeof LAB_THEMES)[number];

export const DEFAULT_THEME: LabTheme = "graphite";

export function isLabTheme(value: string | null | undefined): value is LabTheme {
  return value === "graphite" || value === "amber" || value === "ice";
}

export function parseTheme(value: string | null | undefined): LabTheme {
  return isLabTheme(value) ? value : DEFAULT_THEME;
}

/** 主题统一挂在 `<html data-theme>` 上，与设计语言 §1.3 一致。 */
export function applyTheme(theme: LabTheme): void {
  if (typeof document === "undefined") {
    return;
  }

  document.documentElement.dataset.theme = theme;
}
