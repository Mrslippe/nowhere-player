import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import fixture from "../../../../shared/fixtures/terminal-demo.json";
import type { Block } from "../../../../shared/terminal-model";
import { isBlock } from "../../../../shared/terminal-model.guard";
import { BlockItem } from "./BlockItem";

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

describe("BlockItem", () => {
  it("整块：输入行 + 6 种 outputs + footer 都渲染", () => {
    const html = renderToStaticMarkup(<BlockItem block={demoBlock()} />);

    expect(html).toContain('data-status="done"');
    expect(html).toContain('data-block-id="blk_01JB8Z3K7Q"');
    expect(html).toContain("›");
    expect(html).toContain("scan ~/Music --deep");
    expect(html).toContain("1.8s");
    expect(html).toContain("6 项输出 · 输入 play 1 开始播放");

    for (const marker of OUTPUT_MARKERS) {
      expect(html, marker).toContain(marker);
    }
  });

  it("空 input（欢迎块）不渲染空命令行与状态点", () => {
    const block: Block = {
      ...demoBlock(),
      input: "",
      outputs: [],
      footer: null,
    };

    const html = renderToStaticMarkup(<BlockItem block={block} />);

    expect(html).not.toContain("np-blocks-item__input");
    expect(html).not.toContain("np-blocks-item__status");
  });

  it("运行中显示 4px 状态点与运行中文案，不显示耗时与尾注", () => {
    const block: Block = {
      ...demoBlock(),
      status: "running",
      durationMs: null,
      outputs: [],
      footer: null,
    };

    const html = renderToStaticMarkup(<BlockItem block={block} />);

    expect(html).toContain('data-status="running"');
    expect(html).toContain("np-blocks-item__status");
    expect(html).toContain("np-blocks-item__status-dot");
    expect(html).toContain("运行中");
    expect(html).not.toContain("np-spinner");
    expect(html).not.toContain("1.8s");
    expect(html).not.toContain("np-blocks-item__footer");
  });
});
