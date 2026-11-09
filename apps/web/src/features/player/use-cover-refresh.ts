/**
 * 扫描完成 → 封面缓存刷新（T-0033）。
 *
 * 收到"扫描完成"信号后：
 * 1. 只清 `coverSource` 的 `path → Track`（不清 `coverKey → URL`，避免闪白）；
 * 2. 递增 revision，作为 `useTrackMeta` 的依赖，让**当前曲目**重新解析 Track。
 *
 * 返回的 revision 直接传给 `useTrackMeta`；空闲态（trackId 为 null）不会重查。
 */
import { useEffect, useState } from "react";

import { scanBridge } from "../../shared/scan-bridge";
import { coverSource } from "./cover-source";

export function useCoverRefresh(): number {
  const [revision, setRevision] = useState(0);

  useEffect(
    () =>
      scanBridge.subscribe(() => {
        coverSource.clearTrackCache();
        setRevision((value) => value + 1);
      }),
    [],
  );

  return revision;
}
