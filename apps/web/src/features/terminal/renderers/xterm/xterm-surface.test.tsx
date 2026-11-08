import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { Block } from "../../../../shared/terminal-model";
import { blocksToAnsi } from "./blocks-to-ansi";
import { XtermSurface, writeBlocks } from "./XtermSurface";

const demoBlocks: Block[] = [
  {
    schemaVersion: 1,
    id: "blk_test",
    input: "play 1",
    startedAt: 1_762_512_000_000,
    durationMs: 12,
    status: "done",
    outputs: [
      {
        type: "text",
        tone: null,
        spans: [{ text: "正在播放 晴天", bold: true, dim: false, italic: false, color: "accent" }],
      },
    ],
    footer: null,
  },
];

describe("XtermSurface", () => {
  it("服务端渲染输出 data-testid 挂载点，不抛错", () => {
    const html = renderToStaticMarkup(<XtermSurface blocks={demoBlocks} />);

    expect(html).toContain('data-testid="xterm-surface"');
  });

  it("writeBlocks：先 reset 再整段写入 blocksToAnsi 结果", () => {
    const calls: string[] = [];
    const terminal = {
      reset: () => {
        calls.push("reset");
      },
      write: (data: string) => {
        calls.push(`write:${data}`);
      },
    };

    writeBlocks(terminal, demoBlocks);

    expect(calls).toEqual(["reset", `write:${blocksToAnsi(demoBlocks)}`]);
  });

  it("writeBlocks：blocks 变化后重写新内容，不追加旧内容", () => {
    const writes: string[] = [];
    const terminal = {
      reset: () => undefined,
      write: (data: string) => {
        writes.push(data);
      },
    };

    writeBlocks(terminal, demoBlocks);
    writeBlocks(terminal, []);

    expect(writes).toEqual([blocksToAnsi(demoBlocks), ""]);
  });
});
