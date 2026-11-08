/**
 * 播放器小窗的 6 种预览状态。
 * `hover` 不是真实播放状态，只用于截图与视觉验收：强制显示控制键。
 */
export const PLAYER_STATES = [
  "idle",
  "loading",
  "playing",
  "paused",
  "error",
  "hover",
] as const;

export type PlayerState = (typeof PLAYER_STATES)[number];

export const DEFAULT_PLAYER_STATE: PlayerState = "playing";

/** dev 预览面板上的中文标签；URL 参数仍用英文状态名。 */
export const PLAYER_STATE_LABELS: Record<PlayerState, string> = {
  idle: "空闲",
  loading: "加载",
  playing: "播放",
  paused: "暂停",
  error: "错误",
  hover: "悬停",
};

export function isPlayerState(value: unknown): value is PlayerState {
  return (
    typeof value === "string" &&
    (PLAYER_STATES as readonly string[]).includes(value)
  );
}

/**
 * 从查询串读取状态，非法/缺失时回退到播放态。
 * 传入 search 便于单测；不传则读当前 location（SSR 下安全回退）。
 */
export function readPlayerState(search?: string): PlayerState {
  const query =
    search ?? (typeof window === "undefined" ? "" : window.location.search);
  const value = new URLSearchParams(query).get("state");

  return isPlayerState(value) ? value : DEFAULT_PLAYER_STATE;
}
