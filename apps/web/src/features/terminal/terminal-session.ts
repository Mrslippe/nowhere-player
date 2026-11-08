/**
 * 终端会话的纯逻辑（T-0019）：输入分派、块 id、本地错误块。
 *
 * 组件只负责把它接到 React 状态；这里不碰 DOM、不读全局，便于用
 * `renderToStaticMarkup` + 直接调用纯函数做单测（T-0019 §8）。
 */
import type { Block } from "../../shared/terminal-model";
import { SCHEMA_VERSION } from "../../shared/terminal-model";
import type { CommandError, ExecReply } from "../../shared/command-bridge";
import { toCommandError } from "../../shared/command-bridge";

/** 新块 id 的递增序号，保证同一毫秒内多次提交也不撞 id。 */
let sequence = 0;

/**
 * 展示用版本号。
 *
 * 与 `Cargo.toml` 工作区版本 / `tauri.conf.json` 的 `version` 保持一致（当前均为
 * `0.0.0`）。前端没有构建期版本注入点（`vite.config.ts` 本卡禁改），
 * 版本号提升时这里要同步改；后端 `version` 命令是权威值。
 */
export const APP_VERSION = "0.0.0";

/** 首屏欢迎块的固定 id（不随刷新变化，便于测试与截图）。 */
export const WELCOME_BLOCK_ID = "blk_welcome";

/**
 * 生成 `cmd.exec` 的 `blockId`（前端生成，后端原样回填到两次事件里）。
 *
 * 优先 `crypto.randomUUID()`；运行环境没有时退回「时间戳 + 自增序号」，
 * 仍然保证单窗口内唯一。
 */
export function createBlockId(): string {
  const cryptoRef = globalThis.crypto;

  if (typeof cryptoRef?.randomUUID === "function") {
    return `blk_${cryptoRef.randomUUID()}`;
  }

  sequence += 1;
  return `blk_${Date.now().toString(36)}_${sequence.toString(36)}`;
}

/** 输入框一次提交的去向。 */
export type InputAction =
  | { kind: "ignore" }
  | { kind: "clear" }
  | { kind: "exec"; input: string; blockId: string };

/**
 * 输入 → 动作。
 *
 * - 纯空白：不做任何事（不是错误，命令表 §1.1）。
 * - `clear`：**视图操作**，只清本地块，不发 IPC（命令表 §2.1）。
 * - 其余：交给 `cmd.exec`，块 id 在这里生成。
 */
export function planInput(raw: string, createId: () => string): InputAction {
  const input = raw.trim();

  if (input === "") {
    return { kind: "ignore" };
  }

  if (input.toLowerCase() === "clear") {
    return { kind: "clear" };
  }

  return { kind: "exec", input, blockId: createId() };
}

/** `submitInput` 需要的外部能力；由组件用 React 状态实现，测试里用 spy。 */
export interface TerminalRuntime {
  /** 发一条命令，失败抛 `{ code, message }`。 */
  exec(input: string, blockId: string): Promise<ExecReply>;
  createBlockId(): string;
  /** 按 id 覆盖/追加块（与事件流共用 `upsertBlock`）。 */
  applyBlock(block: Block): void;
  /** 清空本地块，不发 IPC。 */
  clear(): void;
}

/**
 * 输入框提交 → 真实链路。
 *
 * 命令本身的失败（`ok: false`）**不在这里补 UI**：后端已经把 `error` 原语
 * 放进了最终块（T-0019 §6）。只有 IPC 层失败（连块都没有）才补一个本地
 * 错误块，如实说明“命令没有送到后端”。
 */
export async function submitInput(
  raw: string,
  runtime: TerminalRuntime,
): Promise<void> {
  const action = planInput(raw, () => runtime.createBlockId());

  if (action.kind === "ignore") {
    return;
  }

  if (action.kind === "clear") {
    runtime.clear();
    return;
  }

  try {
    await runtime.exec(action.input, action.blockId);
  } catch (error) {
    runtime.applyBlock(
      ipcErrorBlock(action.input, action.blockId, toCommandError(error)),
    );
  }
}

/** IPC 失败时的本地错误块（不是命令失败；命令失败由后端块渲染）。 */
export function ipcErrorBlock(
  input: string,
  blockId: string,
  error: CommandError,
  now: number = Date.now(),
): Block {
  return {
    schemaVersion: SCHEMA_VERSION,
    id: blockId,
    input,
    startedAt: now,
    durationMs: null,
    status: "error",
    outputs: [
      {
        type: "error",
        code: error.code,
        message: `命令没有送到后端：${error.message}`,
        hint: "确认 Tauri 运行时已启动，或检查 cmd.exec 是否注册",
        actions: null,
      },
    ],
    footer: null,
  };
}

/**
 * 首屏欢迎块（T-0019 §6）：版本号 + “输入 help 查看命令”。
 *
 * **不发命令**，纯本地构造；`input` 故意留空——没有跑过任何命令。
 */
export function createWelcomeBlock(
  version: string = APP_VERSION,
  now: number = Date.now(),
): Block {
  return {
    schemaVersion: SCHEMA_VERSION,
    id: WELCOME_BLOCK_ID,
    input: "",
    startedAt: now,
    durationMs: null,
    status: "done",
    outputs: [
      {
        type: "text",
        tone: null,
        spans: [
          {
            text: "Nowhere Player",
            bold: true,
            dim: false,
            italic: false,
            color: "accent",
          },
          {
            text: ` ${version}`,
            bold: false,
            dim: false,
            italic: false,
            color: null,
          },
        ],
      },
    ],
    footer: "输入 help 查看命令",
  };
}
