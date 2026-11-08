import { describe, expect, it } from "vitest";

import { FALLBACK_ARTIST, playerSubtitle, trackDisplay } from "./track-meta";

const WINDOWS_PATH = "C:\\Music\\Radiohead\\Kid A\\01 Everything.flac";
const POSIX_PATH = "/music/radiohead/kid-a/01 Everything.flac";

describe("trackDisplay 的回退规则", () => {
  it("有 Track 时使用库里的标题 / 艺人 / 专辑", () => {
    expect(
      trackDisplay(
        { title: "Everything In Its Right Place", artist: "Radiohead", album: "Kid A" },
        WINDOWS_PATH,
      ),
    ).toEqual({
      title: "Everything In Its Right Place",
      artist: "Radiohead",
      album: "Kid A",
    });
  });

  it("字段首尾空白会被去掉；全是空白视为缺失", () => {
    expect(
      trackDisplay({ title: "  Song  ", artist: "  ", album: "  " }, WINDOWS_PATH),
    ).toEqual({ title: "Song", artist: FALLBACK_ARTIST, album: "" });
  });

  it("查不到 Track（null / undefined）时回退到文件名与占位艺人", () => {
    for (const track of [null, undefined]) {
      expect(trackDisplay(track, WINDOWS_PATH)).toEqual({
        title: "01 Everything",
        artist: "本地文件",
        album: "",
      });
    }
    expect(trackDisplay(null, POSIX_PATH)).toEqual({
      title: "01 Everything",
      artist: "本地文件",
      album: "",
    });
  });

  it("Track.title 为空时用文件名，不显示空标题", () => {
    expect(trackDisplay({ title: "", artist: "A", album: "B" }, WINDOWS_PATH).title).toBe(
      "01 Everything",
    );
    expect(
      trackDisplay({ title: "   ", artist: "A", album: "B" }, "/music/plain.wav").title,
    ).toBe("plain");
  });

  it("字段类型不对（数字 / 对象 / 布尔）按缺失处理", () => {
    expect(
      trackDisplay(
        { title: 42, artist: ["Radiohead"], album: { name: "Kid A" } },
        POSIX_PATH,
      ),
    ).toEqual({ title: "01 Everything", artist: "本地文件", album: "" });
  });

  it("路径为空 / null 时标题为空串（idle 由卡片负责文案）", () => {
    expect(trackDisplay(null, null)).toEqual({
      title: "",
      artist: "本地文件",
      album: "",
    });
    expect(trackDisplay(null, "")).toEqual({
      title: "",
      artist: "本地文件",
      album: "",
    });
  });
});

describe("playerSubtitle", () => {
  it("艺人与专辑都有时用 ` · ` 连接", () => {
    expect(playerSubtitle("Radiohead", "Kid A")).toBe("Radiohead · Kid A");
  });

  it("专辑为空时只显示艺人，不留分隔符", () => {
    expect(playerSubtitle("Radiohead", "")).toBe("Radiohead");
    expect(playerSubtitle("Radiohead", "   ")).toBe("Radiohead");
  });
});
