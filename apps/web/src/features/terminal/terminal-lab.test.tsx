import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { App } from "../../app/App";
import fixture from "../../shared/fixtures/terminal-demo.json";
import type { Block } from "../../shared/terminal-model";
import { isBlock } from "../../shared/terminal-model.guard";
import { TerminalLab } from "./TerminalLab";
import { TerminalView } from "./TerminalView";
import { ScorePanel } from "./components/ScorePanel";
import {
  DEFAULT_SCORES,
  type ComparisonScores,
  type ScoreSet,
} from "./components/scoring";
import { parseVariant, type LabVariant } from "./components/variant";
import { runMockCommand } from "./mock-commands";
import { blocksToAnsi } from "./renderers/xterm/blocks-to-ansi";
import { APP_VERSION, WELCOME_BLOCK_ID } from "./terminal-session";

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

function renderLab(
  variant: LabVariant,
  blocks: Block[],
  scores?: ComparisonScores,
): string {
  return renderToStaticMarkup(
    <TerminalLab
      initialVariant={variant}
      initialTheme="graphite"
      initialBlocks={blocks}
      initialScores={scores}
      persist={false}
    />,
  );
}

function scoreSet(value: number): ScoreSet {
  return {
    visual: value,
    interaction: value,
    ime: value,
    performance: value,
    extensibility: value,
  };
}

describe("parseVariant", () => {
  it("识别三种合法视图", () => {
    expect(parseVariant("blocks")).toBe("blocks");
    expect(parseVariant("xterm")).toBe("xterm");
    expect(parseVariant("both")).toBe("both");
  });

  it("未知或缺省的 ?variant= 回落到 blocks（默认真实链路，T-0019）", () => {
    expect(parseVariant(null)).toBe("blocks");
    expect(parseVariant("side-by-side")).toBe("blocks");
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

  it("顶栏是自绘标题栏：整条可拖动，右侧有最小化/隐藏到托盘", () => {
    const html = renderLab("blocks", []);

    expect(html).toContain('data-tauri-drag-region="deep"');
    expect(html).toContain('data-tauri-drag-region="false"');
    expect(html).toContain('aria-label="最小化"');
    expect(html).toContain('aria-label="隐藏到托盘"');
  });

  it("TerminalView 渲染实验室视图，不抛错", () => {
    const html = renderToStaticMarkup(<TerminalView />);

    expect(html).toContain('data-window="terminal"');
    expect(html).toContain('data-testid="terminal-lab"');
  });
});

describe("首屏真实链路（T-0019）", () => {
  it("首屏只有欢迎块：版本号 + 输入 help 查看命令，默认走结构化视图", () => {
    const html = renderToStaticMarkup(<TerminalView />);

    const start = html.indexOf(`data-block-id="${WELCOME_BLOCK_ID}"`);
    expect(start).toBeGreaterThanOrEqual(0);

    // 只截欢迎块附近，避免匹配到输入框 placeholder 里的同一句话。
    const welcome = html.slice(start, start + 600);
    expect(welcome).toContain("Nowhere Player");
    expect(welcome).toContain(APP_VERSION);
    expect(welcome).toContain("输入 help 查看命令");

    // 默认不再是分屏实验室：只有 A 侧，xterm 仍然可通过 ?variant=xterm 进入。
    expect(html).toContain('data-pane="blocks"');
    expect(html).not.toContain('data-pane="xterm"');
    expect(html.match(/data-block-count="1"/g) ?? []).toHaveLength(1);
  });

  it("实验室入口 ?variant=xterm 仍然可用（保留 M0 演示内容）", () => {
    const html = renderLab("xterm", [demoBlock()]);

    expect(html).toContain('data-pane="xterm"');
    expect(html).toContain('data-testid="xterm-surface"');
  });
});

describe("窗口 label 分流（T-0002 的 App 入口）", () => {
  it("terminal label 显示对比实验室", () => {
    const html = renderToStaticMarkup(<App label="terminal" />);

    expect(html).toContain('data-window="terminal"');
    expect(html).toContain('data-testid="terminal-lab"');
  });

  it("player label 不误渲染终端实验室", () => {
    const html = renderToStaticMarkup(<App label="player" />);

    expect(html).toContain('data-window="player"');
    expect(html).not.toContain('data-testid="terminal-lab"');
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

describe("评分面板", () => {
  it("渲染五维权重、双方案总分与领先方", () => {
    const html = renderLab("both", [], {
      blocks: scoreSet(5),
      xterm: scoreSet(1),
    });

    for (const weight of ["30%", "25%", "20%", "15%", "10%"]) {
      expect(html, weight).toContain(weight);
    }
    expect(html).toContain("A 5.00");
    expect(html).toContain("B 1.00");
    expect(html).toContain("领先 4.00 分");
    expect(html).toContain("导出 JSON");
  });

  it("折叠态保留总分与领先方，但不渲染评分表", () => {
    const html = renderToStaticMarkup(
      <ScorePanel
        scores={DEFAULT_SCORES}
        onChange={() => undefined}
        onExport={() => undefined}
        collapsed
        onToggle={() => undefined}
        feedback={null}
      />,
    );

    expect(html).toContain('data-collapsed="true"');
    expect(html).toContain("A 4.30");
    expect(html).toContain("B 3.40");
    expect(html).not.toContain("np-score__table");
  });

  it("首屏默认显示自评基线（A 4.30 / B 3.40）", () => {
    const html = renderLab("blocks", []);

    expect(html).toContain("A 4.30");
    expect(html).toContain("B 3.40");
  });
});
