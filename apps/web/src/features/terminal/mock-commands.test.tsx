import { describe, expect, it } from "vitest";
import type { Block, BlockStatus, Output } from "../../shared/terminal-model";
import { isBlock } from "../../shared/terminal-model.guard";
import { runMockCommand } from "./mock-commands";
import { APP_VERSION } from "./terminal-session";

function outputTypes(block: Block): string[] {
  return block.outputs.map((output: Output) => output.type);
}

interface CommandCase {
  input: string;
  status: BlockStatus;
  outputs: string[];
}

const COMMAND_CASES: CommandCase[] = [
  { input: "scan", status: "done", outputs: ["progress", "table"] },
  { input: "search 晴天", status: "done", outputs: ["table"] },
  { input: "play 1", status: "done", outputs: ["text"] },
  { input: "perm", status: "error", outputs: ["error"] },
  { input: "hint", status: "done", outputs: ["hint"] },
  { input: "cover", status: "done", outputs: ["media"] },
];

describe("浏览器兜底的 help / version（T-0019）", () => {
  it("help 给出可照抄的命令清单，而不是 E_CMD_UNKNOWN", () => {
    const block = runMockCommand("help", { now: 5 });

    expect(block.status).toBe("done");
    expect(block.outputs.length).toBeGreaterThanOrEqual(10);
    expect(block.outputs.every((output) => output.type === "text")).toBe(true);
    expect(block.footer).toContain("mock");
    expect(isBlock(block)).toBe(true);
  });

  it("version 报 APP_VERSION，与欢迎块同一口径", () => {
    const block = runMockCommand("version");

    expect(block.status).toBe("done");
    const serialized = JSON.stringify(block.outputs);
    expect(serialized).toContain("Nowhere Player");
    expect(serialized).toContain(APP_VERSION);
  });
});

describe("runMockCommand", () => {
  for (const testCase of COMMAND_CASES) {
    it(`${testCase.input} 产出 ${testCase.outputs.join(" + ")}，且符合 Block 契约`, () => {
      const block = runMockCommand(testCase.input, { now: 42 });

      expect(block.status).toBe(testCase.status);
      expect(outputTypes(block)).toEqual(testCase.outputs);
      expect(block.startedAt).toBe(42);
      expect(block.durationMs).not.toBeNull();
      expect(block.input).toBe(testCase.input);
      expect(isBlock(block), testCase.input).toBe(true);
    });
  }

  it("scan 的进度条走到 1284/1284，并给出可检索表格", () => {
    const block = runMockCommand("scan");
    const progress = block.outputs[0];

    expect(progress).toMatchObject({
      type: "progress",
      current: 1284,
      total: 1284,
      unit: "首",
    });
    expect(block.outputs[1]).toMatchObject({
      type: "table",
      columns: ["#", "曲名", "艺人", "时长"],
    });
  });

  it("search 晴天 过滤出结果并带来源列", () => {
    const block = runMockCommand("search 晴天");
    const table = block.outputs[0];

    expect(table).toMatchObject({
      type: "table",
      columns: ["#", "来源", "标题", "艺人", "时长"],
    });
    if (table.type !== "table") {
      throw new Error("search 应产出 table");
    }
    expect(table.rows).toEqual([["1", "本地", "晴天", "周杰伦", "4:29"]]);
    expect(block.footer).toContain("1 条结果");
  });

  it("search 无结果时返回空表格，不编造结果", () => {
    const block = runMockCommand("search 不存在的歌");
    const table = block.outputs[0];

    if (table.type !== "table") {
      throw new Error("search 应产出 table");
    }
    expect(table.rows).toEqual([]);
    expect(block.footer).toContain("0 条结果");
  });

  it("perm 返回 E_LIB_PERMISSION，状态为 error", () => {
    const block = runMockCommand("perm");
    const error = block.outputs[0];

    expect(block.status).toBe("error");
    expect(error).toMatchObject({
      type: "error",
      code: "E_LIB_PERMISSION",
      hint: "检查文件权限，或输入 scan --skip-private",
      actions: null,
    });
    expect(block.actions).toEqual([{ label: "重试", input: "perm" }]);
  });

  it("play 序号越界时返回 E_CMD_ARG，而不是假装播放", () => {
    const block = runMockCommand("play 99");

    expect(block.status).toBe("error");
    expect(block.outputs[0]).toMatchObject({ type: "error", code: "E_CMD_ARG" });
    expect(block.actions).toEqual([{ label: "重试", input: "play 99" }]);
  });

  it("cover 返回 media/cover，尺寸与 fixture 一致", () => {
    const block = runMockCommand("cover");

    expect(block.outputs[0]).toMatchObject({
      type: "media",
      kind: "cover",
      size: 48,
    });
  });

  it("未知命令返回 E_CMD_UNKNOWN 与 did you mean 'scan'?", () => {
    const block = runMockCommand("frobnicate");

    expect(block.status).toBe("error");
    expect(block.outputs[0]).toMatchObject({
      type: "error",
      code: "E_CMD_UNKNOWN",
      hint: "did you mean 'scan'?",
    });
  });

  it("空输入也按未知命令处理（输入框本身会拦截空提交）", () => {
    const block = runMockCommand("   ");

    expect(block.status).toBe("error");
    expect(block.outputs[0]).toMatchObject({ type: "error", code: "E_CMD_UNKNOWN" });
  });

  it("同一个时间戳连续生成两个块，id 仍然唯一", () => {
    const first = runMockCommand("scan", { now: 7 });
    const second = runMockCommand("scan", { now: 7 });

    expect(first.id).not.toBe(second.id);
  });

  it("支持显式覆盖 id，便于测试与稳定截图", () => {
    const block = runMockCommand("hint", { now: 1, id: "blk_fixed" });

    expect(block.id).toBe("blk_fixed");
    expect(isBlock(block)).toBe(true);
  });
});
