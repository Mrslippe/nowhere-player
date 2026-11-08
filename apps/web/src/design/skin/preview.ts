/*
 * dev 预览开关（T-0028）。
 *
 * `?skin=phosphor` 切结构皮肤，并默认配套 phosphor 主题；
 * `?theme=graphite|amber|ice|phosphor` 可单独指定配色，便于验证「结构 / 颜色分离」。
 *
 * 本模块挂在 App 的 import 列表最前，必须在 design/index.ts 之前求值：
 * 主题持久化会把无法识别的 `?theme=phosphor` 回写成 graphite，这里先记下 URL 原值。
 * 生产构建固定 plain、不覆盖主题（主题加载器留给 T-0029）。
 */
export const PREVIEW_SKINS = ["plain", "phosphor"] as const;
export type PreviewSkin = (typeof PREVIEW_SKINS)[number];

export const PREVIEW_THEMES = [
  "graphite",
  "amber",
  "ice",
  "phosphor",
] as const;
export type PreviewTheme = (typeof PREVIEW_THEMES)[number];

export function isPreviewTheme(
  value: string | null | undefined,
): value is PreviewTheme {
  return (
    typeof value === "string" &&
    (PREVIEW_THEMES as readonly string[]).includes(value)
  );
}

type Preview = { skin: PreviewSkin; theme: PreviewTheme | null };

function readPreview(): Preview {
  if (!import.meta.env.DEV || typeof window === "undefined") {
    return { skin: "plain", theme: null };
  }

  let params: URLSearchParams;
  try {
    params = new URLSearchParams(window.location.search);
  } catch {
    return { skin: "plain", theme: null };
  }

  const skin: PreviewSkin = params.get("skin") === "phosphor" ? "phosphor" : "plain";
  const requested = params.get("theme");

  // 未显式指定配色时，phosphor 皮肤默认配磷光主题；plain 保持用户已存主题。
  const theme: PreviewTheme | null = isPreviewTheme(requested)
    ? requested
    : skin === "phosphor"
      ? "phosphor"
      : null;

  return { skin, theme };
}

const preview = readPreview();

/** 首帧前落到 `<html data-skin>` 的默认值；生产构建恒为 plain。 */
export const previewSkin: PreviewSkin = preview.skin;

/** 首帧后需要覆盖的主题；`null` 表示沿用主题持久化的结果。 */
export const previewTheme: PreviewTheme | null = preview.theme;
