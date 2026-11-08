import { describe, expect, it, vi } from "vitest";
import type { Block } from "../../shared/terminal-model";
import { isBlock } from "../../shared/terminal-model.guard";
import {
  createBlockId,
  ipcErrorBlock,
  planInput,
  submitInput,
  type TerminalRuntime,
} from "./terminal-session";

function makeRuntime() {
  const applied: Block[] = [];
  const exec = vi.fn(async (_input: string, blockId: string) => ({
    blockId,
    ok: true,
  }));
  const createId = vi.fn(() => "blk_fixed");
  let clearCount = 0;

  const runtime: TerminalRuntime = {
    exec,
    createBlockId: createId,
    applyBlock: (block) => {
      applied.push(block);
    },
    clear: () => {
      clearCount += 1;
    },
  };

  return { runtime, applied, exec, createId, clearCount: () => clearCount };
}

describe("planInput", () => {
  it("纯空白 = 什么都不做，也不生成块 id", () => {
    const createId = vi.fn(() => "blk_unused");

    expect(planInput("   ", createId)).toEqual({ kind: "ignore" });
    expect(createId).not.toHaveBeenCalled();
  });

  it("clear 大小写不敏感，且不生成块 id（本地视图操作）", () => {
    const createId = vi.fn(() => "blk_unused");

    expect(planInput("clear", createId)).toEqual({ kind: "clear" });
    expect(planInput("  CLEAR  ", createId)).toEqual({ kind: "clear" });
    expect(createId).not.toHaveBeenCalled();
  });

  it("普通输入去掉首尾空白后交给 exec", () => {
    const action = planInput("  play 1 ", () => "blk_7");

    expect(action).toEqual({ kind: "exec", input: "play 1", blockId: "blk_7" });
  });

  it("带参数的 clear 不是本地清屏（交给后端报参数错误）", () => {
    const action = planInput("clear extra", () => "blk_8");

    expect(action).toMatchObject({ kind: "exec", input: "clear extra" });
  });
});

describe("submitInput（clear 不发 IPC）", () => {
  it("clear 只清本地块，exec 一次都不调用", async () => {
    const { runtime, exec, clearCount } = makeRuntime();

    await submitInput("clear", runtime);

    expect(exec).not.toHaveBeenCalled();
    expect(clearCount()).toBe(1);
  });

  it("空输入不发命令也不清屏", async () => {
    const { runtime, exec, clearCount, applied } = makeRuntime();

    await submitInput("  ", runtime);

    expect(exec).not.toHaveBeenCalled();
    expect(clearCount()).toBe(0);
    expect(applied).toHaveLength(0);
  });

  it("普通输入用 { input, blockId } 发一次命令，块交给事件流", async () => {
    const { runtime, exec, applied } = makeRuntime();

    await submitInput("play 1", runtime);

    expect(exec).toHaveBeenCalledTimes(1);
    expect(exec).toHaveBeenCalledWith("play 1", "blk_fixed");
    expect(applied).toHaveLength(0);
  });

  it("后端回执 ok=false 不额外补块（错误已在最终块里）", async () => {
    const applied: Block[] = [];
    const runtime: TerminalRuntime = {
      exec: async (_input, blockId) => ({ blockId, ok: false }),
      createBlockId: () => "blk_no",
      applyBlock: (block) => {
        applied.push(block);
      },
      clear: () => undefined,
    };

    await submitInput("frobnicate", runtime);

    expect(applied).toHaveLength(0);
  });

  it("IPC 本身失败时补一个本地错误块，code 透传", async () => {
    const { runtime, applied } = makeRuntime();
    runtime.exec = async () => {
      throw { code: "E_UNKNOWN", message: "invoke failed" };
    };

    await submitInput("help", runtime);

    expect(applied).toHaveLength(1);
    expect(applied[0]).toMatchObject({
      id: "blk_fixed",
      input: "help",
      status: "error",
      outputs: [
        {
          type: "error",
          code: "E_UNKNOWN",
          message: "命令没有送到后端：invoke failed",
        },
      ],
    });
  });
});

describe("ipcErrorBlock", () => {
  it("符合 Block 契约，且 hint 说明去哪查", () => {
    const block = ipcErrorBlock(
      "help",
      "blk_err",
      { code: "E_UNKNOWN", message: "boom" },
      9,
    );

    expect(isBlock(block)).toBe(true);
    expect(block.startedAt).toBe(9);
    expect(block.outputs[0]).toMatchObject({
      type: "error",
      hint: "确认 Tauri 运行时已启动，或检查 cmd.exec 是否注册",
    });
  });
});

describe("createBlockId", () => {
  it("连续生成互不重复的 id", () => {
    const ids = new Set(Array.from({ length: 200 }, () => createBlockId()));

    expect(ids.size).toBe(200);
  });
});
