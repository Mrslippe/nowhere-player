import { describe, expect, it } from "vitest";
import {
  clampPosition,
  coverSeedForTrack,
  extrapolatePosition,
  formatVolume,
  isSeekable,
  isTickStale,
  positionFromSeekRatio,
  previewStateFromStatus,
  progressRatio,
  seekRatioFromPointer,
  trackTitle,
} from "./playback";

const DURATION = 216_000;

describe("clampPosition", () => {
  it("钳制在 [0, duration]", () => {
    expect(clampPosition(-1, DURATION)).toBe(0);
    expect(clampPosition(1000, DURATION)).toBe(1000);
    expect(clampPosition(999_999, DURATION)).toBe(DURATION);
  });

  it("时长未知时只保证非负", () => {
    expect(clampPosition(1000, null)).toBe(1000);
    expect(clampPosition(-5, null)).toBe(0);
    expect(clampPosition(1000, 0)).toBe(1000);
  });

  it("NaN 回退 0", () => {
    expect(clampPosition(Number.NaN, DURATION)).toBe(0);
  });
});

describe("extrapolatePosition（rAF 外推的核心）", () => {
  const anchor = { positionMs: 1000, atMs: 10_000 };

  it("播放中按经过时间外推", () => {
    expect(extrapolatePosition(anchor, 10_500, DURATION, "playing")).toBe(1500);
  });

  it("结果被时长钳制在 [0, duration]", () => {
    expect(
      extrapolatePosition({ positionMs: DURATION - 100, atMs: 0 }, 5_000, DURATION, "playing"),
    ).toBe(DURATION);
    expect(
      extrapolatePosition({ positionMs: 0, atMs: 1000 }, 500, DURATION, "playing"),
    ).toBe(0);
  });

  it("非播放态不外推（暂停后进度停住）", () => {
    expect(extrapolatePosition(anchor, 60_000, DURATION, "paused")).toBe(1000);
    expect(extrapolatePosition(anchor, 60_000, DURATION, "ended")).toBe(1000);
    expect(extrapolatePosition(anchor, 60_000, DURATION, "idle")).toBe(1000);
    expect(extrapolatePosition(anchor, 60_000, DURATION, "error")).toBe(1000);
  });

  it("60fps 连续外推每帧前进，不是 1Hz 跳格", () => {
    const frames = Array.from({ length: 60 }, (_, index) =>
      extrapolatePosition(anchor, 10_000 + index * 16.67, DURATION, "playing"),
    );

    expect(new Set(frames).size).toBeGreaterThan(50);
    expect(frames[1] - frames[0]).toBeCloseTo(16.67, 1);
    expect(frames[59] - frames[0]).toBeCloseTo(983.53, 1);
  });

  it("时长未知时仍能外推，且不为负", () => {
    expect(extrapolatePosition(anchor, 10_250, null, "playing")).toBe(1250);
  });
});

describe("progressRatio（--np-progress 的唯一来源）", () => {
  it("0 / 中点 / 结束三档", () => {
    expect(progressRatio(0, DURATION)).toBe(0);
    expect(progressRatio(DURATION / 2, DURATION)).toBe(0.5);
    expect(progressRatio(DURATION, DURATION)).toBe(1);
  });

  it("位置越界钳到 0 / 1", () => {
    expect(progressRatio(-1, DURATION)).toBe(0);
    expect(progressRatio(DURATION + 1, DURATION)).toBe(1);
    expect(progressRatio(Number.NEGATIVE_INFINITY, DURATION)).toBe(0);
    // 复用 clampPosition 的既有语义：非有限位置统一回 0，不把 Infinity 当“结尾”。
    expect(progressRatio(Number.POSITIVE_INFINITY, DURATION)).toBe(0);
    expect(progressRatio(Number.NaN, DURATION)).toBe(0);
  });

  it("时长未知 / 为 0 / NaN 时回 0，不产生除零", () => {
    expect(progressRatio(1000, null)).toBe(0);
    expect(progressRatio(1000, 0)).toBe(0);
    expect(progressRatio(1000, Number.NaN)).toBe(0);
  });
});

describe("拖动进度条的坐标换算", () => {
  it("指针落在轨道内按比例取值", () => {
    expect(seekRatioFromPointer(150, 100, 200)).toBeCloseTo(0.25);
    expect(seekRatioFromPointer(100, 100, 200)).toBe(0);
    expect(seekRatioFromPointer(300, 100, 200)).toBe(1);
  });

  it("越界、零宽、非法值都钳制到 0–1", () => {
    expect(seekRatioFromPointer(50, 100, 200)).toBe(0);
    expect(seekRatioFromPointer(500, 100, 200)).toBe(1);
    expect(seekRatioFromPointer(150, 100, 0)).toBe(0);
    expect(seekRatioFromPointer(Number.NaN, 100, 200)).toBe(0);
  });

  it("比例换算成毫秒并钳制", () => {
    expect(positionFromSeekRatio(0.5, DURATION)).toBe(DURATION / 2);
    expect(positionFromSeekRatio(-1, DURATION)).toBe(0);
    expect(positionFromSeekRatio(2, DURATION)).toBe(DURATION);
    expect(positionFromSeekRatio(0.5, null)).toBe(0);
  });
});

describe("isTickStale（seek 后丢弃旧 tick）", () => {
  it("与目标相差超过容差视为旧值", () => {
    expect(isTickStale(1000, 60_000, 1500)).toBe(true);
    expect(isTickStale(59_500, 60_000, 1500)).toBe(false);
    expect(isTickStale(61_400, 60_000, 1500)).toBe(false);
  });
});

describe("trackTitle", () => {
  it("Windows 绝对路径取文件名并去掉扩展名", () => {
    expect(trackTitle("C:\\Music\\Demo\\晴天.flac")).toBe("晴天");
    expect(trackTitle("C:\\Music\\Demo\\a.b.c.mp3")).toBe("a.b.c");
  });

  it("POSIX 路径同样可用", () => {
    expect(trackTitle("/home/u/Music/晴天.ogg")).toBe("晴天");
    expect(trackTitle("relative/track.wav")).toBe("track");
  });

  it("无扩展名、无路径、空值", () => {
    expect(trackTitle("track")).toBe("track");
    expect(trackTitle("C:\\Music\\noext")).toBe("noext");
    expect(trackTitle(null)).toBe("");
  });
});

describe("coverSeedForTrack", () => {
  it("同一曲目稳定，不同曲目不同", () => {
    expect(coverSeedForTrack("C:\\a\\x.flac")).toBe("x.flac");
    expect(coverSeedForTrack("C:\\b\\y.flac")).not.toBe(coverSeedForTrack("C:\\a\\x.flac"));
    expect(coverSeedForTrack(null)).toBe("nowhere-player");
  });
});

describe("previewStateFromStatus", () => {
  it("冻结状态映射到 6 种展示态", () => {
    expect(previewStateFromStatus("playing")).toBe("playing");
    expect(previewStateFromStatus("paused")).toBe("paused");
    expect(previewStateFromStatus("ended")).toBe("paused");
    expect(previewStateFromStatus("error")).toBe("error");
    expect(previewStateFromStatus("idle")).toBe("idle");
  });
});

describe("formatVolume / isSeekable", () => {
  it("音量读数用 VOL 前缀，0 显示 VOL MUTE", () => {
    expect(formatVolume(0.8)).toBe("VOL 80%");
    expect(formatVolume(1)).toBe("VOL 100%");
    expect(formatVolume(2)).toBe("VOL 100%");
    expect(formatVolume(0)).toBe("VOL MUTE");
    expect(formatVolume(-1)).toBe("VOL MUTE");
    expect(formatVolume(Number.NaN)).toBe("VOL 0%");
  });

  it("有正时长才可拖动", () => {
    const base = {
      status: "paused" as const,
      trackId: "a",
      positionMs: 0,
      durationMs: 1000,
      volume: 1,
      mode: { repeat: "off" as const, shuffle: false },
      error: null,
    };

    expect(isSeekable(base)).toBe(true);
    expect(isSeekable({ ...base, durationMs: null })).toBe(false);
    expect(isSeekable({ ...base, durationMs: 0 })).toBe(false);
  });
});
