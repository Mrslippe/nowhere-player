/**
 * 播放器进度与展示的纯函数（T-0014）。
 *
 * 位置事件只有 4 Hz，直接渲染会看到阶梯；这里把「最后收到的位置 + 本地时钟」
 * 作为锚点，交给 rAF 每帧外推，每收到新值时重新校准（不累加，避免漂移）。
 * 所有函数都是纯函数，便于单测。
 */
import type { PlaybackState, PlaybackStatus } from "../../shared/player-bridge";
import type { PlayerState } from "./state";

/** 位置锚点：某时刻收到的位置。 */
export type PositionAnchor = {
  positionMs: number;
  atMs: number;
};

/** 位置钳制在 `[0, duration]`；时长未知（null / 0 / NaN）时只保证非负。 */
export function clampPosition(ms: number, durationMs: number | null): number {
  if (!Number.isFinite(ms)) {
    return 0;
  }

  const upper =
    durationMs !== null && Number.isFinite(durationMs) && durationMs > 0
      ? durationMs
      : Number.POSITIVE_INFINITY;

  return Math.min(upper, Math.max(0, ms));
}

/**
 * 播放位置 → 0–1 的进度比例（PlayerCard 的 `--np-progress` 只读这个值）。
 *
 * 复用 `clampPosition`：负值 / 超出时长 / NaN 全部钳到边界；时长未知或为 0 时
 * 没有可表达的比例，直接回 0。纯函数，便于覆盖 0 / 1 / 越界三档。
 */
export function progressRatio(
  positionMs: number,
  durationMs: number | null,
): number {
  if (durationMs === null || !Number.isFinite(durationMs) || durationMs <= 0) {
    return 0;
  }

  return clampPosition(positionMs, durationMs) / durationMs;
}

/**
 * 外推当前位置。
 *
 * - `status === "playing"`：`锚点位置 + (now - 锚点时刻)`。
 * - 其他状态：不外推，停在锚点（暂停 / 结束 / 错误 / 空闲）。
 */
export function extrapolatePosition(
  anchor: PositionAnchor,
  nowMs: number,
  durationMs: number | null,
  status: PlaybackStatus,
): number {
  const base = clampPosition(anchor.positionMs, durationMs);

  if (status !== "playing") {
    return base;
  }

  const elapsed = Math.max(0, nowMs - anchor.atMs);

  return clampPosition(base + elapsed, durationMs);
}

/** 指针横坐标 → 0–1 的比例（进度条拖动用）。 */
export function seekRatioFromPointer(
  clientX: number,
  rectLeft: number,
  rectWidth: number,
): number {
  if (!Number.isFinite(clientX) || !Number.isFinite(rectLeft) || rectWidth <= 0) {
    return 0;
  }

  return Math.min(1, Math.max(0, (clientX - rectLeft) / rectWidth));
}

/** 0–1 比例 → 位置（毫秒，已钳制）。 */
export function positionFromSeekRatio(
  ratio: number,
  durationMs: number | null,
): number {
  const duration = durationMs ?? 0;
  if (duration <= 0) {
    return 0;
  }

  const safeRatio = Number.isFinite(ratio) ? Math.min(1, Math.max(0, ratio)) : 0;

  return clampPosition(safeRatio * duration, duration);
}

/**
 * 判断 4 Hz 位置事件是否是 seek 之后才到的旧值。
 *
 * seek 命令返回后到 actor 的下一个 tick 之间，队列里可能还有 seek 之前的位置；
 * 若与刚 seek 的目标相差超过容差，就丢弃，避免「松手后位置跳回」。
 */
export function isTickStale(
  tickMs: number,
  expectedMs: number,
  toleranceMs: number,
): boolean {
  return Math.abs(tickMs - expectedMs) > toleranceMs;
}

/**
 * 从 `trackId` 推导展示标题：本地文件在 T-0013 用绝对路径充当 id，
 * 没有元数据，所以只能显示文件名（去掉扩展名）。
 */
export function trackTitle(trackId: string | null): string {
  if (!trackId) {
    return "";
  }

  const base = trackId.split(/[\\/]/).pop() ?? "";
  const withoutExt = base.replace(/\.[^.\\/]+$/, "");

  return withoutExt || base;
}

/** 封面占位 seed：同一曲目稳定得到同一渐变。 */
export function coverSeedForTrack(trackId: string | null): string {
  const base = trackId ? (trackId.split(/[\\/]/).pop() ?? trackId) : "";

  return base || "nowhere-player";
}

/** 冻结的 `PlaybackStatus` → T-0004 的 6 种展示态（`ended` 按暂停态展示）。 */
export function previewStateFromStatus(status: PlaybackStatus): PlayerState {
  switch (status) {
    case "playing":
      return "playing";
    case "paused":
      return "paused";
    case "ended":
      return "paused";
    case "error":
      return "error";
    case "idle":
    default:
      return "idle";
  }
}

/** 音量（0–1）→ 百分比文案。 */
export function formatVolume(volume: number): string {
  if (!Number.isFinite(volume)) {
    return "0%";
  }

  return `${Math.round(Math.min(1, Math.max(0, volume)) * 100)}%`;
}

/** 播放中是否允许拖动 / 键盘跳转。 */
export function isSeekable(state: PlaybackState): boolean {
  return (state.durationMs ?? 0) > 0;
}
