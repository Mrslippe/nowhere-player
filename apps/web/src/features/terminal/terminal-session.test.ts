import { describe, expect, it, vi } from "vitest";
import type { Block } from "../../shared/terminal-model";
import { isBlock } from "../../shared/terminal-model.guard";
import { createCommandBridge, upsertBlock } from "../../shared/command-bridge";
import {
  APP_VERSION,
  STREAM_LOST_BLOCK_ID,
  STREAM_LOST_MESSAGE,
  WELCOME_BLOCK_ID,
  createBlockId,
  createWelcomeBlock,
  ipcErrorBlock,
  loadAppVersion,
  planInput,
  submitInput,
  withStreamLost,
  withWelcomeVersion,
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

describe("createWelcomeBlock（首屏欢迎块）", () => {
  it("含版本号与 help 提示，且符合 Block 契约", () => {
    const block = createWelcomeBlock("0.0.0", 123);

    expect(isBlock(block)).toBe(true);
    expect(block.id).toBe(WELCOME_BLOCK_ID);
    // 欢迎块没有跑过命令，所以 input 为空（不发 IPC）。
    expect(block.input).toBe("");
    expect(block.startedAt).toBe(123);
    expect(block.status).toBe("done");
    expect(block.footer).toBe("输入 help 查看命令");
    expect(block.outputs[0]).toMatchObject({ type: "text", tone: null });

    const serialized = JSON.stringify(block.outputs);
    expect(serialized).toContain("Nowhere Player");
    expect(serialized).toContain("0.0.0");
  });

  it("默认用 APP_VERSION，且没有耗时（不是一次真实执行）", () => {
    const block = createWelcomeBlock();

    expect(JSON.stringify(block.outputs)).toContain(APP_VERSION);
    expect(block.durationMs).toBeNull();
  });
});

describe("createBlockId", () => {
  it("连续生成互不重复的 id", () => {
    const ids = new Set(Array.from({ length: 200 }, () => createBlockId()));

    expect(ids.size).toBe(200);
  });
});

describe("订阅失败提示（T-0021 §2.2）", () => {
  it("推入一条 E_STREAM_LOST 本地错误块，文案说明实时输出不可用", () => {
    const blocks = withStreamLost(
      [],
      { code: "E_UNKNOWN", message: "listen failed" },
      7,
    );

    expect(blocks).toHaveLength(1);
    expect(isBlock(blocks[0])).toBe(true);
    expect(blocks[0]).toMatchObject({
      id: STREAM_LOST_BLOCK_ID,
      input: "",
      status: "error",
      startedAt: 7,
    });
    expect(blocks[0].outputs[0]).toMatchObject({
      type: "error",
      code: "E_STREAM_LOST",
      message: STREAM_LOST_MESSAGE,
    });
    // 底层原因放 hint，便于排查是权限还是运行时问题。
    expect(JSON.stringify(blocks[0].outputs)).toContain("listen failed");
  });

  it("只提示一次：重复失败不刷屏，且不制造新数组", () => {
    const once = withStreamLost([], undefined, 7);
    const twice = withStreamLost(once, undefined, 8);

    expect(twice).toBe(once);
    expect(twice).toHaveLength(1);
  });

  it("订阅失败链路（假 listen 注入错误）：块只出现一次", async () => {
    const bridge = createCommandBridge({
      transport: {
        async exec(args) {
          return { blockId: args.blockId, ok: true };
        },
        async listenBlocks() {
          throw { code: "E_UNKNOWN", message: "listen 被拒绝" };
        },
      },
    });
    let blocks: Block[] = [];

    bridge.subscribeBlocks(
      (block) => {
        blocks = upsertBlock(blocks, block);
      },
      () => {
        blocks = withStreamLost(blocks);
      },
    );

    await vi.waitFor(() => expect(blocks).toHaveLength(1));
    expect(blocks[0].id).toBe(STREAM_LOST_BLOCK_ID);
    expect(blocks[0].outputs[0]).toMatchObject({
      type: "error",
      code: "E_STREAM_LOST",
    });
  });
});

describe("版本号（T-0021 §2.4）", () => {
  it("读到版本号就用它（可带空白）", async () => {
    expect(await loadAppVersion(async () => "1.2.3")).toBe("1.2.3");
    expect(await loadAppVersion(async () => "  1.2.3  ")).toBe("1.2.3");
  });

  it("读取失败回退兜底版本（浏览器 mock / 非 Tauri）", async () => {
    const failing = async (): Promise<string> => {
      throw new Error("window.__TAURI_INTERNALS__ is undefined");
    };

    expect(await loadAppVersion(failing)).toBe(APP_VERSION);
    expect(APP_VERSION).toBe("0.0.0");
  });

  it("空版本号回退兜底版本，不展示空串", async () => {
    expect(await loadAppVersion(async () => "")).toBe(APP_VERSION);
    expect(await loadAppVersion(async () => "   ")).toBe(APP_VERSION);
  });

  it("拿到真版本后替换欢迎块，保留位置、startedAt 与其它块", () => {
    const welcome = createWelcomeBlock("0.0.0", 123);
    const other = ipcErrorBlock(
      "help",
      "blk_err",
      { code: "E_UNKNOWN", message: "boom" },
      124,
    );

    const blocks = withWelcomeVersion([welcome, other], "1.2.3");

    expect(blocks).toHaveLength(2);
    expect(blocks[0].startedAt).toBe(123);
    expect(JSON.stringify(blocks[0].outputs)).toContain("1.2.3");
    expect(blocks[1]).toBe(other);
  });

  it("版本获取失败时欢迎块仍渲染（兜底版本 + help 提示）", () => {
    const blocks = withWelcomeVersion([createWelcomeBlock("0.0.0", 5)], APP_VERSION);

    expect(blocks).toHaveLength(1);
    expect(blocks[0].id).toBe(WELCOME_BLOCK_ID);
    expect(blocks[0].footer).toBe("输入 help 查看命令");
    const serialized = JSON.stringify(blocks[0].outputs);
    expect(serialized).toContain("Nowhere Player");
    expect(serialized).toContain(APP_VERSION);
  });

  it("clear 之后欢迎块已不在，异步版本到达也不会把它加回来", () => {
    expect(withWelcomeVersion([], "1.2.3")).toEqual([]);
  });
});
