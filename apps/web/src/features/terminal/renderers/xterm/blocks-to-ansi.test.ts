import { describe, expect, it } from "vitest";
import fixture from "../../../../shared/fixtures/terminal-demo.json";
import { isBlock } from "../../../../shared/terminal-model.guard";
import type { Block, Output } from "../../../../shared/terminal-model";
import { blocksToAnsi, displayWidth } from "./blocks-to-ansi";

const ESC = "\u001b";
const stripAnsi = (text: string): string => text.replace(/\u001b\[[0-9;]*m/g, "");

function makeBlock(overrides: Partial<Block> = {}): Block {
  return {
    schemaVersion: 1,
    id: "blk_test",
    input: "scan ~/Music",
    startedAt: 1_762_512_000_000,
    durationMs: null,
    status: "done",
    outputs: [],
    footer: null,
    ...overrides,
  };
}

function blockWith(...outputs: Output[]): Block {
  return makeBlock({ outputs });
}

const bar = (filled: number): string => `[${"█".repeat(filled)}${"░".repeat(20 - filled)}]`;

describe("blocksToAnsi", () => {
  it("text：逐 span 输出语义样式，剥离 ANSI 后仍是原文", () => {
    const ansi = blocksToAnsi([
      blockWith({
        type: "text",
        tone: null,
        spans: [
          { text: "扫描完成：", bold: true, dim: false, italic: false, color: "accent" },
          { text: "1,284 首", bold: false, dim: false, italic: false, color: "success" },
          { text: " · 耗时 1.8s", bold: false, dim: true, italic: false, color: "muted" },
        ],
      }),
    ]);

    expect(stripAnsi(ansi)).toBe("› scan ~/Music\n扫描完成：1,284 首 · 耗时 1.8s\n");
    expect(ansi).toContain(`${ESC}[1m`);
    expect(ansi).toContain(`${ESC}[2m`);
    expect(ansi).toContain(`${ESC}[38;5;111m`);
    expect(ansi).toContain(`${ESC}[38;5;114m`);
    expect(ansi).toContain(`${ESC}[38;5;244m`);
  });

  it("text：tone 作为整段默认色，span.color 覆盖 tone", () => {
    const ansi = blocksToAnsi([
      blockWith({
        type: "text",
        tone: "warn",
        spans: [
          { text: "警告", bold: false, dim: false, italic: false, color: null },
          { text: "已处理", bold: false, dim: false, italic: false, color: "success" },
        ],
      }),
    ]);

    expect(stripAnsi(ansi)).toBe("› scan ~/Music\n警告已处理\n");
    expect(ansi).toContain(`${ESC}[38;5;179m警告`);
    expect(ansi).toContain(`${ESC}[38;5;114m已处理`);
  });

  it("table：列宽按显示宽度（CJK 记 2）对齐，表头 dim，末列不留尾随空格", () => {
    const ansi = blocksToAnsi([
      blockWith({
        type: "table",
        columns: ["曲名", "艺人"],
        rows: [
          ["晴天", "周杰伦"],
          ["夜航", "Lin"],
        ],
        selectable: true,
      }),
    ]);

    const lines = stripAnsi(ansi).split("\n");
    expect(lines).toEqual(["› scan ~/Music", "曲名  艺人", "晴天  周杰伦", "夜航  Lin", ""]);
    expect(ansi).toContain(`${ESC}[2m曲名  艺人`);
    expect(lines.every((line) => !line.endsWith(" "))).toBe(true);
  });

  it("displayWidth：中英混排按码点区间近似（CJK 2、ASCII 1）", () => {
    expect(displayWidth("晴天ab")).toBe(6);
    expect(displayWidth("scan ~/Music")).toBe(12);
    expect(displayWidth("")).toBe(0);
  });

  it("progress：固定 20 格、按比例填充，结尾不加换行", () => {
    const full = blocksToAnsi([
      blockWith({ type: "progress", label: "扫描 ~/Music", current: 1284, total: 1284, unit: "首" }),
    ]);
    expect(stripAnsi(full)).toBe(`› scan ~/Music\n扫描 ~/Music ${bar(20)} 100%`);
    expect(stripAnsi(full).endsWith("\n")).toBe(false);

    const third = blocksToAnsi([
      blockWith({ type: "progress", label: "下载", current: 1, total: 3, unit: "项" }),
    ]);
    expect(stripAnsi(third)).toBe(`› scan ~/Music\n下载 ${bar(6)} 33%`);
  });

  it("progress：总量未知（total 0）不除零", () => {
    const ansi = blocksToAnsi([
      blockWith({ type: "progress", label: "扫描", current: 3, total: 0, unit: "" }),
    ]);
    expect(stripAnsi(ansi)).toBe(`› scan ~/Music\n扫描 ${bar(0)} 0%`);
  });

  it("error：首行红 ✗ 错误码，次行缩进消息，hint 第三行 dim", () => {
    const message = "无法读取目录 ~/Music/Private";
    const hint = "检查文件权限，或输入 scan --skip-private";
    const ansi = blocksToAnsi([
      blockWith({
        type: "error",
        code: "E_LIB_PERMISSION",
        message,
        hint,
        actions: ["重试", "跳过该目录"],
      }),
    ]);

    expect(stripAnsi(ansi)).toBe(`› scan ~/Music\n✗ E_LIB_PERMISSION\n  ${message}\n  ${hint}\n`);
    expect(ansi).toContain(`${ESC}[38;5;203m✗ E_LIB_PERMISSION`);
    expect(ansi).toContain(`${ESC}[2m${hint}`);
  });

  it("hint：· 前缀 + dim 文本", () => {
    const ansi = blocksToAnsi([
      blockWith({ type: "hint", text: "did you mean 'play'?", suggestion: "按 Tab 采纳" }),
    ]);

    expect(stripAnsi(ansi)).toBe("› scan ~/Music\n· did you mean 'play'?\n");
    expect(ansi).toContain(`${ESC}[2mdid you mean 'play'?`);
  });

  it("media：降级为 dim 的文字占位", () => {
    const ansi = blocksToAnsi([
      blockWith(
        { type: "media", kind: "cover", ref: "np://library/album/7f3c/cover.jpg", size: 48 },
        { type: "media", kind: "thumb", ref: "np://library/track/1/thumb.jpg", size: 32 },
      ),
    ]);

    expect(stripAnsi(ansi)).toBe("› scan ~/Music\n[cover]\n[thumb]\n");
    expect(ansi).toContain(`${ESC}[2m[cover]`);
    expect(ansi).toContain(`${ESC}[2m[thumb]`);
  });

  it("输入行：error 状态末尾追加 ✗；footer 作为 dim 尾注", () => {
    const ansi = blocksToAnsi([
      makeBlock({ input: "play 99", status: "error", footer: "6 项输出 · 输入 play 1 开始播放" }),
    ]);

    expect(stripAnsi(ansi)).toBe("› play 99 ✗\n6 项输出 · 输入 play 1 开始播放\n");
    expect(ansi).toContain(`${ESC}[1m› play 99 ✗`);
    expect(ansi).toContain(`${ESC}[2m6 项输出`);
  });

  it("未知 Output.type 按 ARCH-005 §7.3 降级为 [unsupported]", () => {
    const unknown = { type: "chart", series: [1, 2, 3] } as unknown as Output;
    const ansi = blocksToAnsi([blockWith(unknown)]);

    expect(stripAnsi(ansi)).toBe("› scan ~/Music\n[unsupported]\n");
  });

  it("每行都以 \\x1b[0m 复位", () => {
    const ansi = blocksToAnsi([
      blockWith(
        {
          type: "text",
          tone: "accent",
          spans: [{ text: "一行", bold: true, dim: false, italic: false, color: null }],
        },
        { type: "table", columns: ["a"], rows: [["b"]], selectable: false },
        { type: "hint", text: "提示", suggestion: null },
      ),
    ]);

    const lines = ansi.split("\n").filter((line) => line.length > 0);
    expect(lines.length).toBeGreaterThan(0);
    expect(lines.every((line) => line.endsWith(`${ESC}[0m`))).toBe(true);
  });

  it("幂等：同一输入两次调用结果完全相同（含多块拼接）", () => {
    const blocks: Block[] = [
      makeBlock({ id: "blk_b", input: "queue", status: "running", outputs: [
        { type: "progress", label: "同步", current: 2, total: 5, unit: "首" },
      ] }),
      makeBlock({ id: "blk_a", input: "scan", outputs: [
        { type: "text", tone: null, spans: [{ text: "ok", bold: false, dim: false, italic: false, color: "success" }] },
      ] }),
    ];

    const first = blocksToAnsi(blocks);
    expect(blocksToAnsi(blocks)).toBe(first);
    // 进度块后还有块时，必须补换行，不能把两块拼到同一行。
    expect(stripAnsi(first)).toContain("40%\n› scan");
  });

  it("terminal-demo.json：六种原语都能落到 ANSI 文本", () => {
    if (!isBlock(fixture)) {
      throw new Error("terminal-demo.json 不符合 Block 契约");
    }

    const text = stripAnsi(blocksToAnsi([fixture]));

    expect(text.startsWith("› scan ~/Music --deep\n")).toBe(true);
    expect(text).toContain("扫描完成：1,284 首 · 耗时 1.8s");
    expect(text).toContain("晴天      周杰伦  4:29");
    expect(text).toContain(`扫描 ~/Music ${bar(20)} 100%`);
    expect(text).toContain("✗ E_LIB_PERMISSION");
    expect(text).toContain("· did you mean 'play'?");
    expect(text).toContain("[cover]");
    expect(text).toContain("6 项输出 · 输入 play 1 开始播放");
  });
});
