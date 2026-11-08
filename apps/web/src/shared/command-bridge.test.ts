import { describe, expect, it, vi } from "vitest";
import type { Block } from "./terminal-model";
import {
  CMD_EXEC_COMMAND,
  TERMINAL_BLOCK_EVENT,
  createCommandBridge,
  isTauriRuntime,
  toCommandError,
  upsertBlock,
  type BlockHandler,
  type CommandError,
  type CommandTransport,
  type ExecArgs,
} from "./command-bridge";

function makeBlock(overrides: Partial<Block> = {}): Block {
  return {
    schemaVersion: 1,
    id: "blk_1",
    input: "help",
    startedAt: 1,
    durationMs: null,
    status: "running",
    outputs: [],
    footer: null,
    ...overrides,
  };
}

/** 假 emitter：记录订阅与调用参数，测试里手动 emit 块（不依赖 Tauri 运行时）。 */
function createFakeTransport(): {
  transport: CommandTransport;
  calls: ExecArgs[];
  emit: (block: Block) => void;
  subscribed: () => boolean;
} {
  const calls: ExecArgs[] = [];
  let handler: BlockHandler | null = null;

  return {
    calls,
    emit: (block) => handler?.(block),
    subscribed: () => handler !== null,
    transport: {
      async exec(args) {
        calls.push(args);
        return { blockId: args.blockId, ok: true };
      },
      async listenBlocks(next) {
        handler = next;
        return () => {
          handler = null;
        };
      },
    },
  };
}

describe("wire 名（T-0018 契约）", () => {
  it("命令名带点、事件名带冒号", () => {
    expect(CMD_EXEC_COMMAND).toBe("cmd.exec");
    expect(TERMINAL_BLOCK_EVENT).toBe("terminal:block");
  });
});

describe("createCommandBridge（真实通道）", () => {
  it("exec 用 { input, blockId } 调用命令通道并透传回执", async () => {
    const { transport, calls } = createFakeTransport();
    const bridge = createCommandBridge({ transport });

    const reply = await bridge.exec("queue list", "blk_42");

    expect(bridge.isMock).toBe(false);
    expect(calls).toEqual([{ input: "queue list", blockId: "blk_42" }]);
    expect(reply).toEqual({ blockId: "blk_42", ok: true });
  });

  it("subscribeBlocks 收到的事件块原样交给 handler", async () => {
    const { transport, emit } = createFakeTransport();
    const bridge = createCommandBridge({ transport });
    const received: Block[] = [];

    bridge.subscribeBlocks((block) => received.push(block));
    emit(makeBlock({ status: "running" }));
    emit(makeBlock({ status: "done", durationMs: 5 }));

    expect(received.map((block) => block.status)).toEqual(["running", "done"]);
  });

  it("事件先于 exec 回执到达：块不丢，按 id 只保留一条最终块", async () => {
    let handler: BlockHandler | null = null;
    const running = makeBlock({ id: "blk_cmd" });
    const done = makeBlock({
      id: "blk_cmd",
      status: "done",
      durationMs: 3,
      footer: "完",
    });

    const transport: CommandTransport = {
      async exec(args) {
        // 后端在返回回执前就 emit（T-0018 的 cmd_exec 顺序）。
        handler?.(running);
        handler?.(done);
        return { blockId: args.blockId, ok: true };
      },
      async listenBlocks(next) {
        handler = next;
        return () => {
          handler = null;
        };
      },
    };

    const bridge = createCommandBridge({ transport });
    let blocks: Block[] = [];
    bridge.subscribeBlocks((block) => {
      blocks = upsertBlock(blocks, block);
    });

    const reply = await bridge.exec("play 1", "blk_cmd");

    expect(reply.ok).toBe(true);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toMatchObject({ id: "blk_cmd", status: "done", durationMs: 3 });
  });

  it("exec 等订阅建立完成才发命令：subscribed() 为真时通道被调用", async () => {
    const { transport, calls, subscribed } = createFakeTransport();
    const bridge = createCommandBridge({ transport });

    bridge.subscribeBlocks(() => undefined);
    expect(subscribed()).toBe(true);

    await bridge.exec("help", "blk_1");
    expect(calls).toHaveLength(1);
  });

  it("exec 会等异步建立中的订阅，首条命令也不丢块", async () => {
    let handler: BlockHandler | null = null;
    let emittedBeforeListen = false;
    const block = makeBlock({ id: "blk_first", status: "done", durationMs: 0 });

    const transport: CommandTransport = {
      async exec(args) {
        if (handler === null) {
          emittedBeforeListen = true;
        }
        handler?.(block);
        return { blockId: args.blockId, ok: true };
      },
      async listenBlocks(next) {
        await Promise.resolve();
        await Promise.resolve();
        handler = next;
        return () => {
          handler = null;
        };
      },
    };

    const bridge = createCommandBridge({ transport });
    const received: Block[] = [];
    bridge.subscribeBlocks((next) => received.push(next));

    await bridge.exec("help", "blk_first");

    expect(emittedBeforeListen).toBe(false);
    expect(received).toHaveLength(1);
    expect(received[0].id).toBe("blk_first");
  });

  it("取消订阅后不再收到块，通道被拆掉", async () => {
    const { transport, emit, subscribed } = createFakeTransport();
    const bridge = createCommandBridge({ transport });
    const received: Block[] = [];

    const off = bridge.subscribeBlocks((block) => received.push(block));
    emit(makeBlock({ status: "running" }));
    off();
    emit(makeBlock({ status: "done" }));

    expect(received).toHaveLength(1);

    // 通道拆除跟在 listen 建立的微任务之后，等它落地再断言。
    await Promise.resolve();
    expect(subscribed()).toBe(false);
  });

  it("订阅失败时 onError 收到一次归一化错误，不向调用方抛", async () => {
    const transport: CommandTransport = {
      async exec(args) {
        return { blockId: args.blockId, ok: true };
      },
      async listenBlocks() {
        throw { code: "E_EVENT_DENIED", message: "listen 被拒绝" };
      },
    };
    const bridge = createCommandBridge({ transport });
    const errors: CommandError[] = [];

    bridge.subscribeBlocks(
      () => undefined,
      (error) => errors.push(error),
    );

    await vi.waitFor(() => expect(errors).toHaveLength(1));
    expect(errors[0]).toEqual({
      code: "E_EVENT_DENIED",
      message: "listen 被拒绝",
    });
  });

  it("订阅失败后 exec 不卡在 ready 上：命令仍能发出并拿到回执", async () => {
    const calls: ExecArgs[] = [];
    const transport: CommandTransport = {
      async exec(args) {
        calls.push(args);
        return { blockId: args.blockId, ok: true };
      },
      async listenBlocks() {
        throw new Error("listen failed");
      },
    };
    const bridge = createCommandBridge({ transport });
    const errors: CommandError[] = [];

    bridge.subscribeBlocks(
      () => undefined,
      (error) => errors.push(error),
    );
    await vi.waitFor(() => expect(errors).toHaveLength(1));

    const reply = await bridge.exec("help", "blk_after_failure");

    expect(calls).toEqual([{ input: "help", blockId: "blk_after_failure" }]);
    expect(reply.ok).toBe(true);
    // Error 归一成 E_UNKNOWN，保留原文。
    expect(errors[0]).toEqual({ code: "E_UNKNOWN", message: "listen failed" });
  });
});

describe("upsertBlock（按 id 覆盖）", () => {
  it("同 id 覆盖原位，不追加第二条", () => {
    const running = makeBlock({ id: "blk_a", status: "running" });
    const done = makeBlock({ id: "blk_a", status: "done", durationMs: 9 });

    const blocks = upsertBlock(upsertBlock([], running), done);

    expect(blocks).toHaveLength(1);
    expect(blocks[0].status).toBe("done");
  });

  it("新 id 追加到末尾，已有块的顺序不变", () => {
    const first = makeBlock({ id: "blk_a", status: "done" });
    const second = makeBlock({ id: "blk_b", status: "done" });

    const blocks = upsertBlock([first], second);

    expect(blocks.map((block) => block.id)).toEqual(["blk_a", "blk_b"]);
  });

  it("不修改传入数组（纯函数）", () => {
    const original = [makeBlock({ id: "blk_a" })];

    upsertBlock(original, makeBlock({ id: "blk_a", status: "done" }));

    expect(original[0].status).toBe("running");
  });
});


describe("浏览器 mock 模式（无 Tauri）", () => {
  it("无 window / 无 Tauri 时 isTauriRuntime 为 false", () => {
    expect(isTauriRuntime()).toBe(false);
  });

  it("exec 出块并派发给订阅者，回执 ok 与状态一致", async () => {
    const bridge = createCommandBridge({
      fallback: (input, blockId) =>
        makeBlock({ id: blockId, input, status: "done", durationMs: 2 }),
    });
    const received: Block[] = [];
    bridge.subscribeBlocks((block) => received.push(block));

    const reply = await bridge.exec("help", "blk_mock");

    expect(bridge.isMock).toBe(true);
    expect(reply).toEqual({ blockId: "blk_mock", ok: true });
    expect(received).toHaveLength(1);
    expect(received[0]).toMatchObject({ id: "blk_mock", input: "help", status: "done" });
  });

  it("error 块的回执 ok=false（失败也只是一块，不额外弹 UI）", async () => {
    const bridge = createCommandBridge({
      fallback: (input, blockId) =>
        makeBlock({ id: blockId, input, status: "error", outputs: [] }),
    });

    const reply = await bridge.exec("frobnicate", "blk_err");

    expect(reply.ok).toBe(false);
  });

  it("没注入 fallback 时 exec 抛 E_UNSUPPORTED，不假装成功", async () => {
    const bridge = createCommandBridge();

    await expect(bridge.exec("help", "blk_none")).rejects.toMatchObject({
      code: "E_UNSUPPORTED",
    });
  });
});

describe("toCommandError", () => {
  it("保留 { code, message }，Error 归一成 E_UNKNOWN", () => {
    expect(toCommandError({ code: "E_BUSY", message: "忙" })).toEqual({
      code: "E_BUSY",
      message: "忙",
    });
    expect(toCommandError(new Error("炸了"))).toEqual({
      code: "E_UNKNOWN",
      message: "炸了",
    });
  });
});
