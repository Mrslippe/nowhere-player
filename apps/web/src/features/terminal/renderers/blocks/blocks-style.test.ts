import { describe, expect, it } from "vitest";

/*
 * 读文件用 Node 内置模块。apps/web 没有安装 @types/node（红线：不新增依赖），
 * 所以用动态导入 + 窄接口补类型，运行时由 Vitest（Node 环境）解析。
 */
type FsApi = {
  readFileSync: (path: string, encoding: "utf8") => string;
  readdirSync: (
    path: string,
    options: { recursive: true; withFileTypes: true },
  ) => Array<{ name: string; parentPath: string; isFile: () => boolean }>;
};
type PathApi = { join: (...parts: string[]) => string };

const fs = (await import("node:fs" as string)) as FsApi;
const path = (await import("node:path" as string)) as PathApi;
const { fileURLToPath } = (await import("node:url" as string)) as {
  fileURLToPath: (url: URL) => string;
};

const BLOCKS_DIR = fileURLToPath(new URL(".", import.meta.url));
const blocksCss = fs.readFileSync(path.join(BLOCKS_DIR, "blocks.css"), "utf8");

function blockSourceFiles(): string[] {
  return fs
    .readdirSync(BLOCKS_DIR, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => path.join(entry.parentPath, entry.name))
    .filter((file) => /\.(css|ts|tsx)$/.test(file));
}

describe("blocks 渲染器静态约束", () => {
  it("组件与样式不含 #RRGGBB 色值字面量", () => {
    const offenders = blockSourceFiles()
      .filter((file) =>
        /#[0-9a-fA-F]{3,8}\b/.test(fs.readFileSync(file, "utf8")),
      )
      .map((file) => file.slice(BLOCKS_DIR.length + 1));

    expect(offenders).toEqual([]);
  });

  it("颜色声明只引用 CSS 变量", () => {
    const declarations = [...blocksCss.matchAll(/(?:^|[\s;{])color:\s*([^;]+);/g)]
      .map((match) => match[1].trim())
      .filter(Boolean);

    expect(declarations.length).toBeGreaterThan(0);
    expect(
      declarations.filter((value) => !value.startsWith("var(--")),
    ).toEqual([]);
  });

  it("prefers-reduced-motion 下取消过渡", () => {
    expect(blocksCss).toContain("@media (prefers-reduced-motion: reduce)");
    expect(blocksCss).toMatch(
      /@media \(prefers-reduced-motion: reduce\)[\s\S]*transition:\s*none/,
    );
  });
});
