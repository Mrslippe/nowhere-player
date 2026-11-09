import { describe, expect, it } from "vitest";

import { parseLab } from "./lab";

describe("parseLab", () => {
  it("?lab=1 打开实验台，?lab=0 / 缺省关闭", () => {
    expect(parseLab("?lab=1")).toBe(true);
    expect(parseLab("?foo=1&lab=1")).toBe(true);
    expect(parseLab("?lab=0")).toBe(false);
    expect(parseLab("?lab=true")).toBe(false);
    expect(parseLab("")).toBe(false);
    expect(parseLab(null)).toBe(false);
    expect(parseLab(undefined)).toBe(false);
  });

  it("兼容 M0 的 ?variant= 直链（只用于实验台）", () => {
    expect(parseLab("?variant=xterm")).toBe(true);
    expect(parseLab("?variant=both")).toBe(true);
  });
});
