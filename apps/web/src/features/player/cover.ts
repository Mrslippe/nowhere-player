/**
 * 封面占位：由 seed 派生确定性渐变，不依赖任何图片资源。
 *
 * 哈希用 FNV-1a：稳定、无依赖；同一 seed 永远得到同一组色相/角度，
 * 不同 seed 均匀落在色轮上。颜色用 hsl() 表达，组件里不出现 #RRGGBB 字面量。
 */
export type CoverGradient = {
  /** 渐变起点色。 */
  from: string;
  /** 渐变终点色。 */
  to: string;
  /** 渐变角度（deg）。 */
  angle: number;
};

const FNV_OFFSET_BASIS = 0x811c9dc5;
const FNV_PRIME = 0x01000193;

function hashSeed(seed: string): number {
  let hash = FNV_OFFSET_BASIS;

  for (let index = 0; index < seed.length; index += 1) {
    hash ^= seed.charCodeAt(index);
    hash = Math.imul(hash, FNV_PRIME);
  }

  return hash >>> 0;
}

/** 同一个 seed 永远返回同一渐变。 */
export function coverGradient(seed: string): CoverGradient {
  const hash = hashSeed(seed);

  const hue = hash % 360;
  const hueShift = 24 + ((hash >>> 9) % 48);
  const saturation = 38 + ((hash >>> 5) % 15);
  const lightnessStart = 32 + ((hash >>> 13) % 8);
  const lightnessEnd = 20 + ((hash >>> 21) % 8);
  const angle = 105 + ((hash >>> 17) % 6) * 15;

  return {
    from: `hsl(${hue}, ${saturation}%, ${lightnessStart}%)`,
    to: `hsl(${(hue + hueShift) % 360}, ${saturation}%, ${lightnessEnd}%)`,
    angle,
  };
}

/** 可直接塞进 style.backgroundImage 的渐变。 */
export function coverBackground(seed: string): string {
  const { from, to, angle } = coverGradient(seed);
  return `linear-gradient(${angle}deg, ${from}, ${to})`;
}

/** 封面中央的专辑首字；缺专辑名时用音符兜底。 */
export function coverInitial(album: string): string {
  const trimmed = album.trim();

  if (!trimmed) {
    return "♪";
  }

  return Array.from(trimmed)[0] ?? "♪";
}
