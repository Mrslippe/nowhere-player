import { useState } from "react";
import type { CSSProperties } from "react";
import type {
  MediaKind,
  MediaOutput as MediaOutputModel,
} from "../../../../../shared/terminal-model";

const DEFAULT_SIZE = 48;
const PLACEHOLDER_ANGLES = [90, 135, 180, 225] as const;

function isAbsoluteUrl(value: string): boolean {
  try {
    new URL(value);
    return true;
  } catch {
    return false;
  }
}

/** 确定性渐变占位：同一个 ref 永远得到同一条渐变（不 import 播放器目录）。 */
function placeholderStyle(ref: string, kind: MediaKind): CSSProperties {
  let hash = 0;

  for (const character of `${kind}:${ref}`) {
    hash = (hash * 31 + (character.codePointAt(0) ?? 0)) >>> 0;
  }

  const angle = PLACEHOLDER_ANGLES[hash % PLACEHOLDER_ANGLES.length];

  return {
    backgroundImage: `linear-gradient(${angle}deg, var(--accent-soft), var(--bg-elev))`,
  };
}

export type MediaOutputProps = {
  output: MediaOutputModel;
};

/**
 * `media` 原语：正方形圆角图。`ref` 是 URL 时用 `<img>`，
 * 非 URL 或加载失败时退化为渐变占位（与 xterm 的文字占位不同）。
 */
export function MediaOutput({ output }: MediaOutputProps) {
  const [failed, setFailed] = useState(false);
  const kind: MediaKind = output.kind === "thumb" ? "thumb" : "cover";
  const label = kind === "cover" ? "封面" : "缩略图";
  const size =
    Number.isFinite(output.size) && output.size > 0
      ? Math.round(output.size)
      : DEFAULT_SIZE;
  const showImage = !failed && isAbsoluteUrl(output.ref);
  const boxStyle: CSSProperties = { width: size, height: size };

  return (
    <div className="np-blocks-media">
      {showImage ? (
        <img
          className="np-blocks-media__image"
          src={output.ref}
          alt={label}
          width={size}
          height={size}
          loading="lazy"
          decoding="async"
          onError={() => setFailed(true)}
          style={boxStyle}
        />
      ) : (
        <span
          className="np-blocks-media__placeholder"
          role="img"
          aria-label={`${label}占位`}
          style={{ ...boxStyle, ...placeholderStyle(output.ref, kind) }}
        />
      )}
    </div>
  );
}
