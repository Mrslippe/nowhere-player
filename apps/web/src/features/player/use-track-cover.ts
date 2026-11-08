/**
 * 当前曲目的封面 URL（T-0025）。
 *
 * 只负责把 `PlaybackState.trackId`（本地文件绝对路径）解析成 `<img src>`；
 * 解析是异步的，解析完成前返回 `null`，界面继续显示确定性渐变。
 * 失败与“无封面”都由 `coverSource` 缓存成 `null`，切歌不会反复重试。
 */
import { useEffect, useState } from "react";

import { coverSource } from "./cover-source";

/** 路径为 `null`（idle / 未知）时直接返回 `null`。 */
export function useTrackCoverSrc(
  trackId: string | null | undefined,
): string | null {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    if (typeof trackId !== "string" || trackId.length === 0) {
      setSrc(null);
      return;
    }

    let disposed = false;
    // 切歌先回到渐变，避免上一首封面短暂残留。
    setSrc(null);
    void coverSource.coverUrlForTrack(trackId).then((url) => {
      if (!disposed) {
        setSrc(url);
      }
    });

    return () => {
      disposed = true;
    };
  }, [trackId]);

  return src;
}
