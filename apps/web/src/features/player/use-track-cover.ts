/**
 * 当前曲目的封面 URL（T-0025）。
 *
 * T-0026 起元数据（标题/艺人/专辑）与封面走同一个 hook：本文件保留为
 * 兼容入口，内部委托 `useTrackMeta`，不再单独订阅 `library.trackByPath`；
 * 缓存仍由 `coverSource` 按路径去重。
 */
import { useTrackMeta } from "./use-track-meta";

/** 路径为 `null`（idle / 未知）或解析失败时返回 `null`。 */
export function useTrackCoverSrc(
  trackId: string | null | undefined,
): string | null {
  return useTrackMeta(trackId).coverSrc;
}
