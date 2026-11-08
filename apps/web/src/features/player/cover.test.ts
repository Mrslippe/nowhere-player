import { describe, expect, it } from "vitest";
import { coverBackground, coverGradient, coverInitial } from "./cover";

describe("coverGradient", () => {
  it("同一个 seed 两次生成完全相同的渐变", () => {
    expect(coverGradient("currents")).toEqual(coverGradient("currents"));
    expect(coverBackground("currents")).toBe(coverBackground("currents"));
  });

  it("不同 seed 生成不同渐变", () => {
    expect(coverGradient("currents")).not.toEqual(coverGradient("yehuimei"));
    expect(coverBackground("currents")).not.toBe(
      coverBackground("yehuimei"),
    );
  });

  it("输出合法的 hsl 颜色，且不含十六进制色值", () => {
    const { from, to } = coverGradient("laodong");
    const hsl = /^hsl\(\d{1,3}, \d{1,2}%, \d{1,2}%\)$/;

    expect(from).toMatch(hsl);
    expect(to).toMatch(hsl);
    expect(from).not.toContain("#");
    expect(to).not.toContain("#");
  });

  it("64 个不同 seed 产出互不相同的背景", () => {
    const backgrounds = new Set(
      Array.from({ length: 64 }, (_, index) => coverBackground(`album-${index}`)),
    );

    expect(backgrounds.size).toBe(64);
  });
});

describe("coverInitial", () => {
  it("取专辑名首字（中文、带空白）", () => {
    expect(coverInitial("叶惠美")).toBe("叶");
    expect(coverInitial("  劳动之余 ")).toBe("劳");
    expect(coverInitial("Currents")).toBe("C");
  });

  it("空专辑名回退音符", () => {
    expect(coverInitial("")).toBe("♪");
    expect(coverInitial("   ")).toBe("♪");
  });

  it("按码点取首字，不拆散 emoji", () => {
    expect(coverInitial("🌊 海")).toBe("🌊");
  });
});
