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

const DESIGN_DIR = fileURLToPath(new URL(".", import.meta.url));

function readDesignFile(relativePath: string): string {
  return fs.readFileSync(path.join(DESIGN_DIR, relativePath), "utf8");
}

/** 任务卡 §6.2 的必填 token 清单。 */
const REQUIRED_TOKENS = [
  "--bg",
  "--bg-elev",
  "--bg-hover",
  "--fg",
  "--fg-muted",
  "--fg-faint",
  "--accent",
  "--accent-soft",
  "--accent-fg",
  "--danger",
  "--warn",
  "--success",
  "--line",
  "--line-strong",
  "--hi",
  "--sp-1",
  "--sp-2",
  "--sp-3",
  "--sp-4",
  "--sp-5",
  "--sp-6",
  "--r-sm",
  "--r-md",
  "--r-lg",
  "--r-xl",
  "--shadow-1",
  "--shadow-2",
  "--shadow-window",
] as const;

const COLOR_TOKENS = [
  "--bg",
  "--bg-elev",
  "--bg-hover",
  "--fg",
  "--fg-muted",
  "--fg-faint",
  "--accent",
  "--accent-soft",
  "--accent-fg",
  "--danger",
  "--warn",
  "--success",
  "--line",
  "--line-strong",
  "--hi",
] as const;

const THEMES = ["graphite", "amber", "ice", "phosphor"] as const;

const tokensCss = readDesignFile("tokens.css");
const themesCss = readDesignFile("themes.css");
const resetCss = readDesignFile("reset.css");
const componentsCss = readDesignFile(path.join("components", "components.css"));

/** token 名只含字母数字和短横线，可直接拼进正则。 */
function declarationsOf(css: string, token: string): number {
  return css.match(new RegExp(`${token}\\s*:`, "g"))?.length ?? 0;
}

// 色值定义文件：tokens.css 是缺省值，themes.css 是三套主题值。
const DEFINITION_FILES = new Set([
  path.join(DESIGN_DIR, "tokens.css"),
  path.join(DESIGN_DIR, "themes.css"),
]);

function designSourceFiles(): string[] {
  return fs
    .readdirSync(DESIGN_DIR, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => path.join(entry.parentPath, entry.name))
    .filter((file) => /\.(css|ts|tsx)$/.test(file));
}

describe("设计 token", () => {
  it("tokens.css 定义了任务卡要求的全部变量名", () => {
    const missing = REQUIRED_TOKENS.filter(
      (token) => declarationsOf(tokensCss, token) === 0,
    );

    expect(missing).toEqual([]);
  });

  it("每套主题各自覆盖全部颜色变量", () => {
    for (const theme of THEMES) {
      expect(themesCss, theme).toContain(`[data-theme="${theme}"]`);
    }

    for (const token of COLOR_TOKENS) {
      expect(declarationsOf(themesCss, token), token).toBe(THEMES.length);
    }
  });

  it("组件与 demo 不含硬编码 #RRGGBB（色值只允许出现在 token 定义文件）", () => {
    const offenders = designSourceFiles()
      .filter((file) => !DEFINITION_FILES.has(file))
      .filter((file) => /#[0-9a-fA-F]{3,8}\b/.test(fs.readFileSync(file, "utf8")))
      .map((file) => file.slice(DESIGN_DIR.length + 1));

    expect(offenders).toEqual([]);
  });

  it("prefers-reduced-motion 下动画与过渡归零", () => {
    expect(resetCss).toContain("@media (prefers-reduced-motion: reduce)");
    expect(resetCss).toContain("transition-duration: 0s !important");
    expect(componentsCss).toContain("@media (prefers-reduced-motion: reduce)");
    expect(componentsCss).toMatch(/\.np-spinner\s*\{[^}]*animation:\s*none/);
  });
});
