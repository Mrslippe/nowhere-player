import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import fixture from "../../../../shared/fixtures/terminal-demo.json";
import type { Block } from "../../../../shared/terminal-model";
import { isBlock } from "../../../../shared/terminal-model.guard";
import { BlocksView } from "./BlocksView";

const OUTPUT_MARKERS = [
  "np-blocks-text",
  "np-blocks-table",
  "np-blocks-progress",
  "np-blocks-error",
  "np-blocks-hint",
  "np-blocks-media",
];

function demoBlock(): Block {
  if (!isBlock(fixture)) {
    throw new Error("terminal-demo.json 不符合 Block 契约");
  }

  return fixture;
}

describe("BlocksView", () => {
  it("用 terminal-demo.json 渲染时 6 种原语全部出现且不报错", () => {
    const html = renderToStaticMarkup(<BlocksView blocks={[demoBlock()]} />);

    expect(html).toContain('aria-label="终端输出"');
    for (const marker of OUTPUT_MARKERS) {
      expect(html, marker).toContain(marker);
    }
  });

  it("未知 Output.type 降级为 text + [unsupported]（ARCH-005 §7.3）", () => {
    const block = {
      ...demoBlock(),
      outputs: [{ type: "chart", series: [1, 2, 3] }],
    } as unknown as Block;

    const html = renderToStaticMarkup(<BlocksView blocks={[block]} />);

    expect(html).toContain("[unsupported]");
    expect(html).toContain("np-blocks-text");
    expect(html).not.toContain("np-blocks-table");
  });

  it("未知 status 降级为 pending（ARCH-005 §7.3）", () => {
    const block = {
      ...demoBlock(),
      status: "paused",
      outputs: [],
    } as unknown as Block;

    const html = renderToStaticMarkup(<BlocksView blocks={[block]} />);

    expect(html).toContain('data-status="pending"');
    expect(html).toContain("等待");
  });
});
