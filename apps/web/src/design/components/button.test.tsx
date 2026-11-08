import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Button } from "./Button";

const VARIANTS = ["primary", "ghost", "danger"] as const;

describe("Button", () => {
  it.each(VARIANTS)("变体 %s 可渲染且带 data-variant", (variant) => {
    const html = renderToStaticMarkup(<Button variant={variant}>播放</Button>);

    expect(html).toContain(`data-variant="${variant}"`);
    expect(html).toContain(`np-btn--${variant}`);
    expect(html).toContain("播放");
  });

  it("默认走 primary / md，且 type=button 避免误提交表单", () => {
    const html = renderToStaticMarkup(<Button>确定</Button>);

    expect(html).toContain('data-variant="primary"');
    expect(html).toContain('data-size="md"');
    expect(html).toContain('type="button"');
  });
});
