/**
 * 当前曲目的元数据（T-0026）：把 `PlaybackState.trackId`（本地绝对路径）
 * 解析成标题 / 艺人 / 专辑 + 封面 URL（T-0025 的封面链路）。
 *
 * 解析是异步的：拿到结果前先按“查不到 Track”同步渲染回退值
 * （文件名 / `本地文件` / 空专辑），切歌不会先闪成空白。
 *
 * `library.trackByPath` 与封面 URL 共用 `coverSource` 里按路径缓存的
 * Promise，因此切一首歌只发一次 `trackByPath`；失败也缓存为 `null`。
 */
import { useEffect, useState } from "react";

import { coverSource } from "./cover-source";
import { trackDisplay } from "./track-meta";
import type { TrackDisplay } from "./track-meta";

/** 播放器卡片需要的完整展示数据。 */
export type TrackMeta = TrackDisplay & {
  /** 真实封面 URL；`null` 时界面显示确定性渐变。 */
  coverSrc: string | null;
};

/** 无 Track 时的同步回退（标题仍用文件名，避免切歌闪空）。 */
function fallbackMeta(trackId: string | null | undefined): TrackMeta {
  return { ...trackDisplay(null, trackId), coverSrc: null };
}

/** 路径为 `null`（idle / 未知）时返回空标题 + 占位艺人。 */
export function useTrackMeta(
  trackId: string | null | undefined,
): TrackMeta {
  const [meta, setMeta] = useState<TrackMeta>(() => fallbackMeta(trackId));

  useEffect(() => {
    if (typeof trackId !== "string" || trackId.length === 0) {
      setMeta(fallbackMeta(null));
      return;
    }

    let disposed = false;
    // 切歌先回到回退值，元数据到了再替换。
    setMeta(fallbackMeta(trackId));
    void Promise.all([
      coverSource.trackForPath(trackId),
      coverSource.coverUrlForTrack(trackId),
    ]).then(([track, coverSrc]) => {
      if (!disposed) {
        setMeta({ ...trackDisplay(track, trackId), coverSrc });
      }
    });

    return () => {
      disposed = true;
    };
  }, [trackId]);

  return meta;
}
