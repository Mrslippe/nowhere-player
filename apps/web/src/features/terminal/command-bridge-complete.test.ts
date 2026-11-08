import { describe, expect, it } from "vitest";
import {
  CMD_COMPLETE_COMMAND,
  createCommandBridge,
  type CommandTransport,
  type CompleteArgs,
} from "../../shared/command-bridge";

function makeTransport(overrides: Partial<CommandTransport> = {}): CommandTransport {
  return {
    async exec(args) {
      return { blockId: args.blockId, ok: true };
    },
    async listenBlocks() {
      return () => undefined;
    },
    ...overrides,
  };
}

describe("cmd.complete 通道（T-0020）", () => {
  it("命令名带点", () => {
    expect(CMD_COMPLETE_COMMAND).toBe("cmd.complete");
  });

  it("把 { input, cursor } 原样交给通道并归一化 camelCase 结果", async () => {
    const calls: CompleteArgs[] = [];
    const bridge = createCommandBridge({
      transport: makeTransport({
        async complete(args) {
          calls.push(args);
          return {
            replaceStart: 0,
            replaceEnd: 2,
            items: [{ text: "play", kind: "command", summary: "播放" }],
          };
        },
      }),
    });

    const completion = await bridge.complete("pl", 2);

    expect(calls).toEqual([{ input: "pl", cursor: 2 }]);
    expect(completion).toEqual({
      replaceStart: 0,
      replaceEnd: 2,
      items: [{ text: "play", kind: "command", summary: "播放" }],
    });
  });

  it("通道没有补全实现时返回空列表（不弹层、不报错）", async () => {
    const bridge = createCommandBridge({ transport: makeTransport() });

    const completion = await bridge.complete("play ", 5);

    expect(completion).toEqual({
      replaceStart: 5,
      replaceEnd: 5,
      items: [],
    });
  });

  it("脏回执降级为空列表：只保留合法候选，未知类型并入 command", async () => {
    const bridge = createCommandBridge({
      transport: makeTransport({
        async complete() {
          // 绕过类型检查，模拟后端字段被改坏的情况。
          return {
            replaceStart: -1,
            replaceEnd: "oops",
            items: [
              null,
              { text: "", kind: "command", summary: "" },
              { text: "play", kind: "mystery", summary: 42 },
            ],
          } as never;
        },
      }),
    });

    const completion = await bridge.complete("pl", 2);

    expect(completion.replaceStart).toBe(2);
    expect(completion.replaceEnd).toBe(2);
    expect(completion.items).toEqual([
      { text: "play", kind: "command", summary: "" },
    ]);
  });

  it("通道失败时抛 { code, message }，调用方据此降级", async () => {
    const bridge = createCommandBridge({
      transport: makeTransport({
        async complete() {
          throw { code: "E_BUSY", message: "播放器线程不可用" };
        },
      }),
    });

    await expect(bridge.complete("play ", 5)).rejects.toEqual({
      code: "E_BUSY",
      message: "播放器线程不可用",
    });
  });

  it("浏览器 mock 桥返回空列表，Tab 不阻塞输入", async () => {
    const bridge = createCommandBridge({});

    expect(bridge.isMock).toBe(true);
    expect(await bridge.complete("pl", 2)).toEqual({
      replaceStart: 2,
      replaceEnd: 2,
      items: [],
    });
  });
});
