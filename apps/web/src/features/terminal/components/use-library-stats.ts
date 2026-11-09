/**
 * 终端顶栏的本地库状态（T-0041）。
 *
 * 首屏读一次 `library.stats`；之后每次扫描完成（T-0033 的 scanBridge 代次）
 * 再读一次，保证「本地 N 首」不是过期数字。浏览器预览 / IPC 失败显示未连接。
 */
import { useEffect, useState } from "react";

import { scanBridge } from "../../../shared/scan-bridge";
import { fetchLibraryStats } from "../../../shared/library-bridge";

export type LibraryStatus = {
  state: "loading" | "ready" | "unavailable";
  tracks: number | null;
};

export function useLibraryStats(): LibraryStatus {
  const [status, setStatus] = useState<LibraryStatus>({
    state: "loading",
    tracks: null,
  });

  useEffect(() => {
    let active = true;

    const load = () => {
      void fetchLibraryStats().then((stats) => {
        if (!active) {
          return;
        }

        setStatus(
          stats
            ? { state: "ready", tracks: stats.tracks }
            : { state: "unavailable", tracks: null },
        );
      });
    };

    load();
    const off = scanBridge.subscribe(load);

    return () => {
      active = false;
      off();
    };
  }, []);

  return status;
}
