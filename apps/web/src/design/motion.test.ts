import { describe, expect, it } from "vitest";

type FsApi = { readFileSync: (path: string, encoding: "utf8") => string };

const fs = (await import("node:fs" as string)) as FsApi;
const { fileURLToPath } = (await import("node:url" as string)) as {
  fileURLToPath: (url: URL) => string;
};

const motionCss = fs.readFileSync(
  fileURLToPath(new URL("./motion.css", import.meta.url)),
  "utf8",
);

describe("动效 token（T-0041）", () => {
  it("时长与缓动集中声明在 motion.css", () => {
    for (const token of [
      "--np-dur-instant",
      "--np-dur-fast",
      "--np-dur-base",
      "--np-dur-slow",
      "--np-ease-standard",
      "--np-ease-decelerate",
      "--np-ease-accelerate",
      "--np-ease-linear",
    ]) {
      expect(motionCss, token).toContain(`${token}:`);
    }
  });

  it("prefers-reduced-motion 下时长归零", () => {
    expect(motionCss).toContain("@media (prefers-reduced-motion: reduce)");
    expect(motionCss).toMatch(/--np-dur-instant:\s*0ms/);
    expect(motionCss).toMatch(/--np-dur-fast:\s*0ms/);
    expect(motionCss).toMatch(/--np-dur-base:\s*0ms/);
    expect(motionCss).toMatch(/--np-dur-slow:\s*0ms/);
  });
});
