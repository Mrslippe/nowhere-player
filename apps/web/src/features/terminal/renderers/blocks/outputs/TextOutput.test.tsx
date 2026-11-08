import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { TextOutput as TextOutputModel } from "../../../../../shared/terminal-model";
import { TextOutput } from "./TextOutput";

const OUTPUT: TextOutputModel = {
  type: "text",
  tone: "warn",
  spans: [
    { text: "注意：", bold: true, dim: false, italic: false, color: "danger" },
    { text: "磁盘剩余 2%", bold: false, dim: true, italic: true, color: null },
  ],
};

describe("TextOutput", () => {
  it("逐 span 渲染，整段 tone 与行内样式都落到结构上", () => {
    const html = renderToStaticMarkup(<TextOutput output={OUTPUT} />);

    expect(html).toContain('data-tone="warn"');
    expect(html).toContain("注意：");
    expect(html).toContain("磁盘剩余 2%");
    expect(html).toContain('data-color="danger"');
    expect(html).toContain('data-bold="true"');
    expect(html).toContain('data-dim="true"');
    expect(html).toContain('data-italic="true"');
    expect(html.match(/<span/g)).toHaveLength(OUTPUT.spans.length);
  });

  it("默认色（tone/color 为 null）不输出多余属性", () => {
    const html = renderToStaticMarkup(
      <TextOutput
        output={{
          type: "text",
          tone: null,
          spans: [
            { text: "普通", bold: false, dim: false, italic: false, color: null },
          ],
        }}
      />,
    );

    expect(html).not.toContain("data-tone");
    expect(html).not.toContain("data-color");
    expect(html).toContain("普通");
  });
});
