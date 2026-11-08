/**
 * 终端命令桥（T-0019）：前端唯一直接碰 `cmd.exec` / `terminal:block` 的地方。
 *
 * 契约来源（T-0018，`apps/desktop/src-tauri/src/command_exec.rs`）：
 * - 命令名 `cmd.exec`（点号），入参 `{ input, blockId }`，**立即**回执 `{ blockId, ok }`。
 *   输出不在回执里，只经事件推送。
 * - 事件名 `terminal:block`（冒号；Tauri 事件名不允许点号）。
 * - 一条命令推两个**同 id** 的块：先 `running`（空输出），再最终块 → 前端按 id 覆盖，
 *   不能追加（见 {@link upsertBlock}）。
 * - 后端在 `cmd.exec` **返回之前** 就 emit，事件可能先于回执到达；订阅必须在发命令之前
 *   建立好，`exec` 会等订阅就绪再发（见 {@link createCommandBridge}）。
 * - 事件是广播到所有窗口的；本模块只该在终端窗订阅。
 *
 * 浏览器（无 Tauri 运行时）退化为 mock：`fallback` 由终端 feature 注入
 * （`shared/` 禁止依赖 `features/`，见 `docs/40-规范/项目结构规范.md` §5）。
 * mock 下 `exec` 同步产出块并派发给本地订阅者，回执形状与真实通道一致。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Block } from "./terminal-model";
import { isBlock } from "./terminal-model.guard";

/** 命令名（点号，T-0018 冻结）。 */
export const CMD_EXEC_COMMAND = "cmd.exec";

/** 事件名（冒号，T-0018 冻结）。 */
export const TERMINAL_BLOCK_EVENT = "terminal:block";

/** 归一化后的失败：UI 只认 `{ code, message }`（与 `player-bridge` 同形）。 */
export interface CommandError {
  code: string;
  message: string;
}

/** `cmd.exec` 的入参（camelCase，Tauri v2 约定）。 */
export interface ExecArgs {
  input: string;
  /** 前端生成的块 id，后端原样用于两次事件的 `id`。 */
  blockId: string;
}

/** `cmd.exec` 的立即回执：只说明成败，输出走事件。 */
export interface ExecReply {
  blockId: string;
  ok: boolean;
}

export type BlockHandler = (block: Block) => void;

export type Unsubscribe = () => void;

/**
 * 浏览器兜底实现：`input + blockId → Block`。
 *
 * 契约上放进 `features/terminal/`（`mock-commands.ts` 的 `runMockCommand`），
 * 这里只留函数类型，避免 `shared/` 反向依赖 `features/`。
 */
export type FallbackRunner = (input: string, blockId: string) => Block;

/**
 * IPC 通道抽象。
 *
 * 生产实现是 Tauri（{@link tauriTransport}）；单测注入假 emitter，不依赖 Tauri 运行时。
 */
export interface CommandTransport {
  exec(args: ExecArgs): Promise<ExecReply>;
  /** 订阅块事件；返回取消函数。 */
  listenBlocks(handler: BlockHandler): Promise<Unsubscribe>;
}

export interface CommandBridgeOptions {
  /** 显式通道（单测用）；缺省时按是否跑在 Tauri 里选择真实/mock。 */
  transport?: CommandTransport;
  /** 无 Tauri 时的本地实现；终端 feature 注入 `runMockCommand` 适配器。 */
  fallback?: FallbackRunner;
}

export interface CommandBridge {
  /** 当前是否处于浏览器 mock 模式。 */
  readonly isMock: boolean;
  /** 发一条命令；失败抛 {@link CommandError}。 */
  exec(input: string, blockId: string): Promise<ExecReply>;
  /** 订阅块事件（含 `running` 与最终块）；返回取消函数，重复调用安全。 */
  subscribeBlocks(handler: BlockHandler): Unsubscribe;
}

/** 当前是否跑在 Tauri 运行时里（浏览器预览为 false）。 */
export function isTauriRuntime(): boolean {
  if (typeof window === "undefined") {
    return false;
  }

  return (
    (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !==
    undefined
  );
}

/** 把任意异常归一成 `{ code, message }`。 */
export function toCommandError(error: unknown): CommandError {
  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error &&
    typeof (error as { code: unknown }).code === "string" &&
    typeof (error as { message: unknown }).message === "string"
  ) {
    const { code, message } = error as CommandError;
    return { code, message };
  }

  if (error instanceof Error) {
    return { code: "E_UNKNOWN", message: error.message };
  }

  return { code: "E_UNKNOWN", message: String(error) };
}

/**
 * 按 `id` 覆盖或追加块（T-0019 §6）。
 *
 * 同一个 `blockId` 会有两次事件（`running` → 最终块）：覆盖而不是追加，
 * 否则一条命令会显示成两行。找不到同 id 时追加到末尾（新命令）。
 */
export function upsertBlock(blocks: readonly Block[], block: Block): Block[] {
  const index = blocks.findIndex((item) => item.id === block.id);

  if (index === -1) {
    return [...blocks, block];
  }

  const next = blocks.slice();
  next[index] = block;
  return next;
}

/** 真实通道：Tauri `invoke` + `listen`。 */
const tauriTransport: CommandTransport = {
  async exec({ input, blockId }) {
    try {
      const raw = await invoke<unknown>(CMD_EXEC_COMMAND, { input, blockId });
      return normalizeReply(raw);
    } catch (error) {
      throw toCommandError(error);
    }
  },

  async listenBlocks(handler) {
    const off = await listen<unknown>(TERMINAL_BLOCK_EVENT, (event) => {
      // 契约校验：形状不对就丢弃，绝不把脏数据渲染进终端。
      if (isBlock(event.payload)) {
        handler(event.payload);
      }
    });

    return off;
  },
};

/** 回执校验：只认 `{ blockId: string, ok: boolean }`。 */
function normalizeReply(raw: unknown): ExecReply {
  if (typeof raw === "object" && raw !== null) {
    const value = raw as Record<string, unknown>;
    if (typeof value.blockId === "string" && typeof value.ok === "boolean") {
      return { blockId: value.blockId, ok: value.ok };
    }
  }

  throw {
    code: "E_UNKNOWN",
    message: "cmd.exec 返回了非法回执",
  } satisfies CommandError;
}

/**
 * 建一个命令桥。
 *
 * 真实模式的关键顺序：**先订阅、再发命令**。`exec` 会等首个订阅的 `listen` 建立完成
 * 才调用 `cmd.exec`，因为后端在返回回执前就 emit 了块；反过来（先 `exec` 再订阅）
 * 首条命令的块会丢（T-0019 §11 风险 1）。
 */
export function createCommandBridge(
  options: CommandBridgeOptions = {},
): CommandBridge {
  if (options.transport === undefined && !isTauriRuntime()) {
    return createMockBridge(options.fallback);
  }

  return createTransportBridge(options.transport ?? tauriTransport);
}

function createMockBridge(fallback: FallbackRunner | undefined): CommandBridge {
  const handlers = new Set<BlockHandler>();

  return {
    isMock: true,

    async exec(input, blockId) {
      if (fallback === undefined) {
        throw {
          code: "E_UNSUPPORTED",
          message: "浏览器预览没有注入 mock 命令实现",
        } satisfies CommandError;
      }

      const block = fallback(input, blockId);

      // 复制一份订阅列表再派发，避免 handler 里取消订阅导致遍历跳过。
      for (const handler of [...handlers]) {
        handler(block);
      }

      return { blockId, ok: block.status !== "error" };
    },

    subscribeBlocks(handler) {
      handlers.add(handler);
      return () => {
        handlers.delete(handler);
      };
    },
  };
}

function createTransportBridge(transport: CommandTransport): CommandBridge {
  const handlers = new Set<BlockHandler>();
  let unsubscribe: Unsubscribe | null = null;
  /** 订阅建立 Promise；`exec` 发命令前先等它。 */
  let ready: Promise<void> | null = null;
  /** 代次：取消订阅后作废仍在途的 `listen`，避免泄漏。 */
  let generation = 0;

  const dispatch = (block: Block): void => {
    for (const handler of [...handlers]) {
      handler(block);
    }
  };

  return {
    isMock: false,

    subscribeBlocks(handler) {
      handlers.add(handler);

      if (ready === null) {
        const current = generation + 1;
        generation = current;
        ready = transport
          .listenBlocks(dispatch)
          .then((off) => {
            if (current !== generation) {
              off();
              return;
            }
            unsubscribe = off;
          })
          .catch(() => {
            // 订阅失败只影响实时块；命令回执与命令失败仍可展示。
            // 不在这里抛：订阅是后台建立的，调用方（组件挂载）没有 await 点。
          });
      }

      return () => {
        handlers.delete(handler);

        if (handlers.size > 0) {
          return;
        }

        // 最后一个订阅者走了：拆掉通道，下次订阅重新建立。
        generation += 1;
        unsubscribe?.();
        unsubscribe = null;
        ready = null;
      };
    },

    async exec(input, blockId) {
      if (ready !== null) {
        await ready;
      }

      try {
        return await transport.exec({ input, blockId });
      } catch (error) {
        throw toCommandError(error);
      }
    },
  };
}
