import { describe, expect, it } from "vitest";

/*
 * 结构层契约（T-0027 / T-0028）。
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
  "--np-border-table-header",
  "--np-border-width-emphasis",
  "--np-shadow-card",
  "--np-shadow-float",
  "--np-highlight-inset",
  "--np-shadow-focus-line",
  "--np-font-ui",
  "--np-font-mono",
  "--np-density-line",
  "--np-density-line-dense",
  "--np-bleed",
] as const;

/** 结构层引用颜色 token 是允许的，但不允许自己定义颜色字面量。 */
const COLOR_LITERAL = /#[0-9a-fA-F]{3,8}\b|rgba?\(|hsla?\(/;

function readSkinFile(name: string): string {
  return fs.readFileSync(path.join(SKIN_DIR, name), "utf8");
}

const baseCss = readSkinFile("base.css");
const plainCss = readSkinFile("plain.css");
const phosphorCss = readSkinFile("phosphor.css");
const themesCss = fs.readFileSync(
  path.join(SRC_DIR, "design", "themes.css"),
  "utf8",
);
const tokensCss = fs.readFileSync(
  path.join(SRC_DIR, "design", "tokens.css"),
  "utf8",
);
const typographyCss = fs.readFileSync(
  path.join(SRC_DIR, "design", "typography.css"),
  "utf8",
);

function declarationsOf(css: string, token: string): number {
  return css.match(new RegExp(`${token}\\s*:`, "g"))?.length ?? 0;
}

function valuesOf(css: string, property: string): string[] {
  return [...css.matchAll(new RegExp(`${property}\\s*:\\s*([^;]+);`, "g"))].map(
    (match) => match[1].trim(),
  );
}

/** 取某个声明的第一个值（用于断言 skin 覆盖值）。 */
function valueOf(css: string, token: string): string | undefined {
  return valuesOf(css, token)[0];
}

/** 抽取 `selector { ... }` 的声明块；selector 里含引号与方括号。 */
function ruleBlock(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = css.match(new RegExp(`${escaped}\\s*\\{([\\s\\S]*?)\\}`));
  return match?.[1] ?? "";
}

function parseHexColor(value: string): [number, number, number] | null {
  const match = value.match(/^#([0-9a-f]{6})$/i);
  if (!match) {
    return null;
  }

  const hex = match[1];
  return [
    Number.parseInt(hex.slice(0, 2), 16),
    Number.parseInt(hex.slice(2, 4), 16),
    Number.parseInt(hex.slice(4, 6), 16),
  ];
}

function relativeLuminance([r, g, b]: [number, number, number]): number {
  const channel = (value: number): number => {
    const normalized = value / 255;
    return normalized <= 0.04045
      ? normalized / 12.92
      : ((normalized + 0.055) / 1.055) ** 2.4;
  };

  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG 2.x 对比度。 */
function contrastRatio(a: string, b: string): number {
  const first = parseHexColor(a);
  const second = parseHexColor(b);
  if (first === null || second === null) {
    throw new Error(`不是 6 位十六进制颜色：${a} / ${b}`);
  }

  const la = relativeLuminance(first);
  const lb = relativeLuminance(second);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

const COMPONENT_CSS = [
  path.join(SRC_DIR, "design", "components", "components.css"),
  path.join(SRC_DIR, "features", "player", "player.css"),
  path.join(SRC_DIR, "features", "terminal", "terminal.css"),
  path.join(SRC_DIR, "features", "terminal", "renderers", "blocks", "blocks.css"),
];

describe("结构层（skin）", () => {
  it("base.css / plain.css / phosphor.css 各自声明全部结构变量", () => {
    for (const [name, css] of [
      ["base.css", baseCss],
      ["plain.css", plainCss],
      ["phosphor.css", phosphorCss],
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
    expect(phosphorCss).not.toMatch(COLOR_LITERAL);
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

  it("phosphor 的结构覆盖符合 T-0028 §6.1", () => {
    // 圆角全部归零，只有状态点是圆的。
    for (const token of [
      "--np-radius-card",
      "--np-radius-inner",
      "--np-radius-control",
      "--np-radius-control-sm",
      "--np-radius-tag",
      "--np-radius-micro",
      "--np-radius-pill",
    ]) {
      expect(valueOf(phosphorCss, token), token).toBe("0");
    }
    expect(valueOf(phosphorCss, "--np-radius-circle")).toBe("50%");

    // 无投影、无卡片边框；控件仍靠 1px 细线分隔。
    expect(valueOf(phosphorCss, "--np-shadow-card")).toBe("none");
    expect(valueOf(phosphorCss, "--np-shadow-float")).toBe("none");
    expect(valueOf(phosphorCss, "--np-border-card")).toBe("none");

    // 全出血 + 等宽 UI + 1.45 行高。
    expect(valueOf(phosphorCss, "--np-bleed")).toBe("1");
    expect(valueOf(phosphorCss, "--np-density-line")).toBe("1.45");
    expect(valueOf(phosphorCss, "--np-font-ui")).toBe("var(--np-font-mono)");
  });

  it("phosphor 不做 CRT 装饰（扫描线 / 辉光留给 M6）", () => {
    expect(phosphorCss).not.toMatch(/repeating-linear-gradient/);
    expect(phosphorCss).not.toMatch(/text-shadow/);
  });

  it("phosphor 主题的 --fg 对 --bg 对比度 ≥ 7:1", () => {
    const block = ruleBlock(themesCss, '[data-theme="phosphor"]');
    const fg = valueOf(block, "--fg");
    const bg = valueOf(block, "--bg");

    expect(fg, "--fg").toBeDefined();
    expect(bg, "--bg").toBeDefined();
    expect(contrastRatio(fg as string, bg as string)).toBeGreaterThanOrEqual(7);
  });

  it("phosphor 主题的 --fg-muted 对 --bg 对比度 ≥ 4.5:1", () => {
    const block = ruleBlock(themesCss, '[data-theme="phosphor"]');
    const muted = valueOf(block, "--fg-muted");
    const bg = valueOf(block, "--bg");

    expect(muted, "--fg-muted").toBeDefined();
    expect(contrastRatio(muted as string, bg as string)).toBeGreaterThanOrEqual(
      4.5,
    );
  });

  it("D3 字阶与字体分工集中在 typography.css", () => {
    for (const token of ["--fs-10", "--fs-11", "--fs-13", "--fs-15"]) {
      expect(typographyCss, token).toContain(`${token}:`);
    }
    for (const token of ["--lh-title", "--lh-body", "--lh-meta", "--lh-micro"]) {
      expect(typographyCss, token).toContain(`${token}:`);
    }

    expect(typographyCss).toContain('"Segoe UI"');
    expect(typographyCss).toContain('"Microsoft YaHei"');
    expect(typographyCss).toContain('"Cascadia Code"');
    expect(typographyCss).toContain("Consolas");
  });

  it("组件 CSS 没有字面量字号，全部走 token", () => {
    for (const file of COMPONENT_CSS) {
      const css = fs.readFileSync(file, "utf8");
      const relative = file.slice(SRC_DIR.length + 1);
      const offenders = valuesOf(css, "font-size").filter(
        (value) => !value.startsWith("var(--"),
      );

      expect(offenders, `${relative} font-size`).toEqual([]);
    }
  });

  it("D3 列宽与间距 token 是 8 的倍数（行高 28 单独例外）", () => {
    for (const token of [
      "--np-space-1",
      "--np-space-2",
      "--np-space-3",
      "--np-space-4",
      "--np-col-index",
      "--np-col-source",
      "--np-col-artist",
      "--np-col-duration",
    ]) {
      const value = valueOf(tokensCss, token);
      expect(value, token).toBeDefined();
      const px = Number.parseInt(value as string, 10);
      expect(px % 8, token).toBe(0);
    }

    expect(valueOf(tokensCss, "--np-row-height")).toBe("28px");
    expect(valueOf(tokensCss, "--np-table-header-line")).toBe("2px");
  });

  it("D3 默认主题的正文/次级/弱文字对比度达标", () => {
    const block = ruleBlock(themesCss, '[data-theme="graphite"]');
    const bg = valueOf(block, "--bg") as string;
    const fg = valueOf(block, "--fg") as string;
    const muted = valueOf(block, "--fg-muted") as string;
    const faint = valueOf(block, "--fg-faint") as string;
    const accent = valueOf(block, "--accent") as string;

    expect(contrastRatio(fg, bg)).toBeGreaterThanOrEqual(12);
    expect(contrastRatio(muted, bg)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(faint, bg)).toBeGreaterThanOrEqual(4.5);
    // 琥珀若用于文字也必须过 4.5（当前只做信号，但先守住）。
    expect(contrastRatio(accent, bg)).toBeGreaterThanOrEqual(4.5);
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

  it("窗口根元素挂了 data-skin=plain（生产默认值）", () => {
    const appSource = fs.readFileSync(path.join(SRC_DIR, "app", "App.tsx"), "utf8");

    expect(appSource).toContain('dataset.skin = "plain"');
    expect(appSource).toContain('dataset.skin = "phosphor"');
  });
});
