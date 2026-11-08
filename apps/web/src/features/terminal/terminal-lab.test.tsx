import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import fixture from "../../shared/fixtures/terminal-demo.json";
import type { Block } from "../../shared/terminal-model";
import { isBlock } from "../../shared/terminal-model.guard";
import { TerminalLab } from "./TerminalLab";
import { parseVariant, type LabVariant } from "./components/variant";
import { runMockCommand } from "./mock-commands";
import { blocksToAnsi } from "./renderers/xterm/blocks-to-ansi";

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

function renderLab(variant: LabVariant, blocks: Block[]): string {
  return renderToStaticMarkup(
    <TerminalLab
      initialVariant={variant}
      initialTheme="graphite"
      initialBlocks={blocks}
      persist={false}
    />,
  );
}

describe("parseVariant", () => {
  it("识别三种合法视图", () => {
    expect(parseVariant("blocks")).toBe("blocks");
    expect(parseVariant("xterm")).toBe("xterm");
    expect(parseVariant("both")).toBe("both");
  });

  it("未知或缺省的 ?variant= 回落到 both", () => {
    expect(parseVariant(null)).toBe("both");
    expect(parseVariant("side-by-side")).toBe("both");
  });
});

describe("TerminalLab 三种视图", () => {
  it("variant=blocks 只挂载结构化渲染器，fixture 的 6 种原语都出现", () => {
    const html = renderLab("blocks", [demoBlock()]);

    for (const marker of OUTPUT_MARKERS) {
      expect(html, marker).toContain(marker);
    }
    expect(html).toContain('data-pane="blocks"');
    expect(html).not.toContain('data-pane="xterm"');
    expect(html).not.toContain('data-testid="xterm-surface"');
  });

  it("variant=xterm 只挂载 xterm 渲染面，不渲染结构化块", () => {
    const html = renderLab("xterm", [demoBlock()]);

    expect(html).toContain('data-pane="xterm"');
    expect(html).toContain('data-testid="xterm-surface"');
    expect(html).not.toContain('data-pane="blocks"');
    expect(html).not.toContain("np-blocks-item");
  });

  it("variant=both 左右两栏拿到同一份 blocks（块数一致）", () => {
    const blocks = [demoBlock(), runMockCommand("search 晴天", { now: 2 })];
    const html = renderLab("both", blocks);

    expect(html).toContain('data-pane="blocks"');
    expect(html).toContain('data-pane="xterm"');
    expect(html).toContain('data-testid="xterm-surface"');
    expect(html).toContain("scan ~/Music --deep");

    const counts = html.match(/data-block-count="2"/g) ?? [];
    expect(counts).toHaveLength(2);
  });

  it("两个窗口 label 共用的视图入口不抛错（TerminalView 挂载 TerminalLab）", () => {
    const html = renderLab("both", []);

    expect(html).toContain('data-testid="terminal-lab"');
    expect(html).toContain('data-variant="both"');
  });
});

describe("perm 命令的双渲染器表现", () => {
  it("A 侧出现 E_LIB_PERMISSION 错误块，B 侧 ANSI 带红色 ✗ E_LIB_PERMISSION", () => {
    const block = runMockCommand("perm", { now: 100 });
    const html = renderLab("both", [block]);

    expect(html).toContain("np-blocks-error");
    expect(html).toContain('data-code="E_LIB_PERMISSION"');
    expect(html).toContain("无法读取目录 ~/Music/Private");

    const ansi = blocksToAnsi([block]);
    expect(ansi).toContain("\u001b[38;5;203m✗ E_LIB_PERMISSION");
    expect(ansi).toContain("检查文件权限");
  });
});
