import { describe, expect, it } from "vitest";

/*
 * 结构层契约（T-0027）。
 *
 * 与 tokens.test.ts 同一套做法：Node 内置模块动态导入，
 * 不给 apps/web 加 @types/node（红线：不新增依赖）。
 */
type FsApi = { readFileSync: (path: string, encoding: "utf8") => string };
type PathApi = { join: (...parts: string[]) => string };

const fs = (await import("node:fs" as string)) as FsApi;
const path = (await import("node:path" as string)) as PathApi;
const { fileURLToPath } = (await import("node:url" as string)) as {
  fileURLToPath: (url: URL) => string;
};

const SKIN_DIR = fileURLToPath(new URL(".", import.meta.url));
const SRC_DIR = path.join(SKIN_DIR, "..", "..");

/** 任务卡 §6.1 的结构变量清单 + 拆分后新增的角色变量。 */
const STRUCTURE_TOKENS = [
  "--np-radius-card",
  "--np-radius-inner",
  "--np-radius-control",
  "--np-radius-control-sm",
  "--np-radius-tag",
  "--np-radius-micro",
  "--np-radius-pill",
  "--np-radius-circle",
  "--np-border-card",
  "--np-border-control",
  "--np-border-control-strong",
  "--np-border-hairline",
  "--np-border-transparent",
  "--np-border-error",
  "--np-border-width-emphasis",
  "--np-shadow-card",
  "--np-shadow-float",
  "--np-shadow-focus-line",
  "--np-font-ui",
  "--np-font-mono",
  "--np-density-line",
  "--np-density-line-dense",
  "--np-bleed",
] as const;

/** 结构层引用颜色 token 是允许的，但不允许自己定义颜色字面量。 */
const COLOR_LITERAL = /#[0-9a-fA-F]{3,8}\b|rgba?\(|hsla?\(/;

const baseCss = fs.readFileSync(path.join(SKIN_DIR, "base.css"), "utf8");
const plainCss = fs.readFileSync(path.join(SKIN_DIR, "plain.css"), "utf8");

function declarationsOf(css: string, token: string): number {
  return css.match(new RegExp(`${token}\\s*:`, "g"))?.length ?? 0;
}

function valuesOf(css: string, property: string): string[] {
  return [...css.matchAll(new RegExp(`${property}\\s*:\\s*([^;]+);`, "g"))].map(
    (match) => match[1].trim(),
  );
}

const COMPONENT_CSS = [
  path.join(SRC_DIR, "design", "components", "components.css"),
  path.join(SRC_DIR, "features", "player", "player.css"),
  path.join(SRC_DIR, "features", "terminal", "terminal.css"),
  path.join(SRC_DIR, "features", "terminal", "renderers", "blocks", "blocks.css"),
];

describe("结构层（skin）", () => {
  it("base.css 与 plain.css 各自声明全部结构变量", () => {
    for (const [name, css] of [
      ["base.css", baseCss],
      ["plain.css", plainCss],
    ] as const) {
      const missing = STRUCTURE_TOKENS.filter(
        (token) => declarationsOf(css, token) === 0,
      );
      expect(missing, name).toEqual([]);
    }
  });

  it("skin 文件只定义结构，不定义颜色字面量", () => {
    expect(baseCss).not.toMatch(COLOR_LITERAL);
    expect(plainCss).not.toMatch(COLOR_LITERAL);
  });

  it("plain.css 只含 --np-* 结构变量声明", () => {
    const properties = [...plainCss.matchAll(/^\s*([a-z-]+)\s*:/gm)].map(
      (match) => match[1],
    );

    expect(properties.length).toBeGreaterThan(0);
    expect(properties.filter((property) => !property.startsWith("--np-"))).toEqual(
      [],
    );
  });

  it("组件 CSS 的圆角/阴影/字体只引用结构变量", () => {
    for (const file of COMPONENT_CSS) {
      const css = fs.readFileSync(file, "utf8");
      const relative = file.slice(SRC_DIR.length + 1);

      for (const property of ["border-radius", "box-shadow", "font-family"]) {
        const offenders = valuesOf(css, property).filter(
          (value) => !value.startsWith("var(--np-"),
        );
        expect(offenders, `${relative} ${property}`).toEqual([]);
      }
    }
  });

  it("组件 CSS 的描边只引用结构变量（0 表示移除，允许）", () => {
    for (const file of COMPONENT_CSS) {
      const css = fs.readFileSync(file, "utf8");
      const relative = file.slice(SRC_DIR.length + 1);
      const offenders = valuesOf(
        css,
        "border(?:-(?:top|right|bottom|left))?",
      ).filter((value) => value !== "0" && !value.startsWith("var(--np-border"));

      expect(offenders, relative).toEqual([]);
    }
  });

  it("窗口根元素挂了 data-skin=plain", () => {
    const appSource = fs.readFileSync(path.join(SRC_DIR, "app", "App.tsx"), "utf8");

    expect(appSource).toContain('dataset.skin = "plain"');
  });
});
