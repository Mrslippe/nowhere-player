/**
 * 当前曲目的元数据（T-0026）：把 `PlaybackState.trackId`（本地绝对路径）
 * 解析成标题 / 艺人 / 专辑 + 封面 URL（T-0025 的封面链路）。
 *
 * 首帧先按“查不到 Track”的回退值渲染（文件名 / `本地文件` / 空专辑）。
 * 切歌时**不立刻清回退值**：保留旧曲目的展示，等新 Track / 封面就绪后
 * 一次性替换，配合 T-0041 的交叉淡入；无旧内容（如从 idle 起播）才用
 * 回退值占位，避免整窗空白。
 *
 * `library.trackByPath` 与封面 URL 共用 `coverSource` 里按路径缓存的
 * Promise，因此切一首歌只发一次 `trackByPath`；失败也缓存为 `null`。
 */
import { useEffect, useRef, useState } from "react";

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
  /** 缓存代次：扫描完成后递增，同曲目重查 Track（不闪回占位封面）。 */
  revision = 0,
): TrackMeta {
  const [meta, setMeta] = useState<TrackMeta>(() => fallbackMeta(trackId));
  const lastTrackRef = useRef<string | null | undefined>(undefined);

  useEffect(() => {
    const track = typeof trackId === "string" && trackId.length > 0 ? trackId : null;

    if (!track) {
      lastTrackRef.current = null;
      setMeta(fallbackMeta(null));
      return;
    }

    let disposed = false;
    // 没有可保留的旧内容（首次起播 / 从 idle 恢复）才先放文件名占位；
    // 正常切歌保留旧曲目，等 T-0041 的淡化随新内容一起发生。
    if (lastTrackRef.current !== track) {
      setMeta((current) =>
        current.title.trim().length === 0 ? fallbackMeta(track) : current,
      );
    }
    lastTrackRef.current = track;

    void Promise.all([
      coverSource.trackForPath(track),
      coverSource.coverUrlForTrack(track),
    ]).then(([record, coverSrc]) => {
      if (!disposed) {
        setMeta({ ...trackDisplay(record, track), coverSrc });
      }
    });

    return () => {
      disposed = true;
    };
  }, [trackId, revision]);

  return meta;
}
