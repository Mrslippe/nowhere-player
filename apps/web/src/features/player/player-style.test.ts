import { describe, expect, it } from "vitest";

/*
 * 与 design/tokens.test.ts 同一套做法：Node 内置模块动态导入，
 * 不给 apps/web 加 @types/node（红线：不新增依赖）。
 */
type FsApi = { readFileSync: (path: string, encoding: "utf8") => string };
type PathApi = { join: (...parts: string[]) => string };

const fs = (await import("node:fs" as string)) as FsApi;
const path = (await import("node:path" as string)) as PathApi;
const { fileURLToPath } = (await import("node:url" as string)) as {
  fileURLToPath: (url: URL) => string;
};

const PLAYER_DIR = fileURLToPath(new URL(".", import.meta.url));

/** 播放器窗口自己的源码：组件与样式里不允许出现色值字面量。 */
const PLAYER_SOURCES = ["PlayerCard.tsx", "PlayerView.tsx", "player.css"];

describe("播放器窗口样式", () => {
  it("组件与样式不含硬编码 #RRGGBB（只用 design token）", () => {
    for (const file of PLAYER_SOURCES) {
      const source = fs.readFileSync(path.join(PLAYER_DIR, file), "utf8");
      expect(source, file).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
    }
  });
});
