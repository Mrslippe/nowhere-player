import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Icon } from "./Icon";
import { IconButton } from "./IconButton";

/*
 * 读 CSS 用动态导入补类型，沿用 tokens.test.ts 的做法：
 * apps/web 没有 @types/node，也不能新增依赖。
 */
type FsApi = { readFileSync: (path: string, encoding: "utf8") => string };
type PathApi = { join: (...parts: string[]) => string };

const fs = (await import("node:fs" as string)) as FsApi;
const path = (await import("node:path" as string)) as PathApi;
const { fileURLToPath } = (await import("node:url" as string)) as {
  fileURLToPath: (url: URL) => string;
};

const COMPONENTS_DIR = fileURLToPath(new URL(".", import.meta.url));
const componentsCss = fs.readFileSync(
  path.join(COMPONENTS_DIR, "components.css"),
  "utf8",
);

/** 取出某个选择器后第一对花括号内的声明，用于断言 CSS 变量值。 */
function blockOf(selector: string): string {
  const start = componentsCss.indexOf(selector);

  if (start < 0) {
    throw new Error(`components.css 缺少选择器 ${selector}`);
  }

  const open = componentsCss.indexOf("{", start);
  const close = componentsCss.indexOf("}", open);

  return componentsCss.slice(open + 1, close);
}

function renderIconButton(size?: "sm" | "md"): string {
  return renderToStaticMarkup(
    <IconButton aria-label="播放" size={size}>
      <Icon name="play" />
    </IconButton>,
  );
}

describe("IconButton", () => {
  it("md（默认）为 32×32，图标 16px", () => {
    const html = renderIconButton();
    const css = blockOf('.np-icon-btn[data-size="md"]');

    expect(html).toContain('data-size="md"');
    expect(css).toContain("--np-icon-btn-size: 32px");
    expect(css).toContain("--np-icon-btn-icon-size: 16px");
  });

  it("sm 为 20×20，图标 14px", () => {
    const html = renderIconButton("sm");
    const css = blockOf('.np-icon-btn[data-size="sm"]');

    expect(html).toContain('data-size="sm"');
    expect(css).toContain("--np-icon-btn-size: 20px");
    expect(css).toContain("--np-icon-btn-icon-size: 14px");
  });

  it("尺寸不进内联样式，且默认 type=button", () => {
    const html = renderIconButton("sm");

    expect(html).not.toContain("style=");
    expect(html).toContain('type="button"');
  });
});
