/**
 * 媒体库统计桥（T-0041）：终端顶栏的「本地库 N 首」状态。
 *
 * 只暴露 `library.stats` 的只读读取；浏览器预览没有 Tauri 时返回 null，
 * 让 UI 显示「本地库未连接」而不是伪造数字。
 */
import { invoke } from "@tauri-apps/api/core";

import { isTauriRuntime } from "./player-bridge";

export type LibraryStats = {
  tracks: number;
  albums: number;
  artists: number;
  totalDurationMs: number;
  lastScanAt: number;
};

function nonNegativeNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

/** 校验 IPC 返回值；字段不完整时当作不可用，不让 NaN 进 UI。 */
export function readLibraryStats(value: unknown): LibraryStats | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }

  const stats = value as Record<string, unknown>;

  if (
    !nonNegativeNumber(stats.tracks) ||
    !nonNegativeNumber(stats.albums) ||
    !nonNegativeNumber(stats.artists) ||
    !nonNegativeNumber(stats.totalDurationMs) ||
    !nonNegativeNumber(stats.lastScanAt)
  ) {
    return null;
  }

  return {
    tracks: stats.tracks,
    albums: stats.albums,
    artists: stats.artists,
    totalDurationMs: stats.totalDurationMs,
    lastScanAt: stats.lastScanAt,
  };
}

/** 读一次媒体库统计；无 Tauri / IPC 失败时回 null。 */
export async function fetchLibraryStats(): Promise<LibraryStats | null> {
  if (!isTauriRuntime()) {
    return null;
  }

  try {
    return readLibraryStats(await invoke<unknown>("library.stats"));
  } catch {
    return null;
  }
}
