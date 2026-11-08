/** demo 页要展示的 token 分组（与 tokens.css 一一对应）。 */
export const COLOR_TOKENS = [
  "--bg",
  "--bg-elev",
  "--bg-hover",
  "--fg",
  "--fg-muted",
  "--fg-faint",
  "--accent",
  "--accent-soft",
  "--accent-fg",
  "--danger",
  "--warn",
  "--success",
  "--line",
  "--line-strong",
] as const;

export const FONT_SIZE_TOKENS = [
  "--fs-10",
  "--fs-12",
  "--fs-13",
  "--fs-15",
  "--fs-20",
] as const;

export const SPACE_TOKENS = [
  "--sp-1",
  "--sp-2",
  "--sp-3",
  "--sp-4",
  "--sp-5",
  "--sp-6",
] as const;

export const RADIUS_TOKENS = [
  "--r-sm",
  "--r-md",
  "--r-lg",
  "--r-xl",
] as const;

export const SHADOW_TOKENS = ["--shadow-1", "--shadow-2"] as const;

export const MOTION_TOKENS = [
  "--dur-fast",
  "--dur-base",
  "--dur-slow",
  "--ease-out",
] as const;

export const ALL_TOKENS = [
  ...COLOR_TOKENS,
  ...FONT_SIZE_TOKENS,
  ...SPACE_TOKENS,
  ...RADIUS_TOKENS,
  ...SHADOW_TOKENS,
  ...MOTION_TOKENS,
] as const;
