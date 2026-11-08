/**
 * 播放器窗的展示字段（T-0026）：库里的 `Track` → 卡片上的标题 / 艺人 / 专辑。
 *
 * `library.trackByPath` 可能 miss（未入库、路径被移动），也可能字段为空，
 * 回退规则集中在这个纯函数里，便于单测与评审：
 *
 * | 字段 | 优先 | 回退 |
 * |---|---|---|
 * | `title` | `Track.title`（去首尾空白） | 文件名去扩展名 |
 * | `artist` | `Track.artist` | `本地文件` |
 * | `album` | `Track.album` | 空串（副标题不显示分隔符） |
 */
import type { TrackRecord } from "./cover-source";
import { trackTitle } from "./playback";

/** 查不到库记录时的艺人占位。 */
export const FALLBACK_ARTIST = "本地文件";

/** 卡片要显示的三个文本字段。 */
export type TrackDisplay = {
  title: string;
  artist: string;
  album: string;
};

/** 只接受非空字符串；其他类型（数字 / null / 对象）一律按缺失处理。 */
function textOf(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

/**
 * 给定库记录（可为 `null`，表示查不到）与播放状态里的路径，算出展示字段。
 *
 * `trackId` 用于标题回退（文件名去扩展名）；传 `null` 时标题为空串。
 */
export function trackDisplay(
  track: TrackRecord | null | undefined,
  trackId: string | null | undefined,
): TrackDisplay {
  const title = textOf(track?.title);
  const artist = textOf(track?.artist);

  return {
    title: title || trackTitle(trackId ?? null),
    artist: artist || FALLBACK_ARTIST,
    album: textOf(track?.album),
  };
}

/** 副标题：专辑为空时只显示艺人，不留 ` · ` 尾巴。 */
export function playerSubtitle(artist: string, album: string): string {
  const trimmedAlbum = album.trim();

  return trimmedAlbum.length > 0 ? `${artist} · ${trimmedAlbum}` : artist;
}
