/**
 * 封面来源（T-0025）：文件路径 / `cover_key` → 可塞进 `<img src>` 的 URL。
 *
 * 数据流（元数据属于媒体库，播放状态只带路径）：
 *
 *   `player:state.trackId`（绝对路径）
 *     → `library.trackByPath` 拿到 `Track.coverKey`
 *     → `library.coverUrl` 拿到封面文件绝对路径
 *     → `convertFileSrc` 拼成当前平台可加载的 URL（**不硬编码** URL 形式）
 *
 * 两级内存缓存都按“一次请求”语义工作：同一个路径 / 同一个 key 只请求一次；
 * 失败（含文件缺失、IPC 报错）也缓存成 `null`，同一张坏图不会反复重试。
 * 缓存结果就是 Promise，因此并发调用天然去重。
 */
import { convertFileSrc, invoke } from "@tauri-apps/api/core";

import { isTauriRuntime } from "../../shared/player-bridge";

/** 封面 key 的形状：16 位小写十六进制（与 Rust 侧校验一致）。 */
const COVER_KEY_PATTERN = /^[0-9a-f]{16}$/;

/** 缓存里存的是“最终是否有 URL”，失败固定为 `null`。 */
export type CoverSource = {
  /** 路径 → `coverKey`；查不到或请求失败为 `null`。 */
  coverKeyForTrack: (path: string | null | undefined) => Promise<string | null>;
  /** `coverKey` → 可加载 URL；key 非法、请求失败或文件缺失为 `null`。 */
  coverUrlForCoverKey: (key: string | null | undefined) => Promise<string | null>;
  /** 路径 → URL，是上面两步的组合。 */
  coverUrlForTrack: (path: string | null | undefined) => Promise<string | null>;
  /** 清空缓存（测试用）。 */
  reset: () => void;
};

/** 可注入的副作用，单测不碰 Tauri。 */
export type CoverSourceDeps = {
  invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
  convertFileSrc: (path: string) => string;
};

/** key 是否合法；非法 key 连 IPC 都不发。 */
export function isCoverKey(value: unknown): value is string {
  return typeof value === "string" && COVER_KEY_PATTERN.test(value);
}

/** 从 `library.trackByPath` 的返回值里取 `coverKey`；形状不对就当作无封面。 */
function readCoverKey(track: unknown): string | null {
  if (typeof track !== "object" || track === null) {
    return null;
  }

  const key = (track as { coverKey?: unknown }).coverKey;

  return isCoverKey(key) ? key : null;
}

/** 用一个依赖集合造一份带缓存的封面来源。 */
export function createCoverSource(deps: CoverSourceDeps): CoverSource {
  const trackKeys = new Map<string, Promise<string | null>>();
  const coverUrls = new Map<string, Promise<string | null>>();

  const coverKeyForTrack = (path: string | null | undefined) => {
    if (typeof path !== "string" || path.length === 0) {
      return Promise.resolve(null);
    }

    const cached = trackKeys.get(path);
    if (cached) {
      return cached;
    }

    const pending = deps
      .invoke("library.trackByPath", { path })
      .then(readCoverKey)
      .catch(() => null);
    trackKeys.set(path, pending);

    return pending;
  };

  const coverUrlForCoverKey = (key: string | null | undefined) => {
    if (!isCoverKey(key)) {
      return Promise.resolve(null);
    }

    const cached = coverUrls.get(key);
    if (cached) {
      return cached;
    }

    const pending = deps
      .invoke("library.coverUrl", { key })
      .then((path) =>
        typeof path === "string" && path.length > 0
          ? deps.convertFileSrc(path)
          : null,
      )
      .catch(() => null);
    coverUrls.set(key, pending);

    return pending;
  };

  const coverUrlForTrack = async (path: string | null | undefined) => {
    const key = await coverKeyForTrack(path);

    return key ? coverUrlForCoverKey(key) : null;
  };

  return {
    coverKeyForTrack,
    coverUrlForCoverKey,
    coverUrlForTrack,
    reset: () => {
      trackKeys.clear();
      coverUrls.clear();
    },
  };
}

/**
 * 生产依赖：浏览器预览没有 Tauri 运行时，直接返回 `null` 让界面走渐变，
 * 而不是抛错刷红点。
 */
const defaultDeps: CoverSourceDeps = {
  invoke: async (command, args) => {
    if (!isTauriRuntime()) {
      return null;
    }

    return invoke<unknown>(command, args);
  },
  convertFileSrc,
};

/** 全应用共用的封面来源（缓存跨曲目保留）。 */
export const coverSource: CoverSource = createCoverSource(defaultDeps);
