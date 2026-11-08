/**
 * 播放器桥（T-0014）：前端唯一直接碰播放器 IPC / 事件的地方。
 *
 * 契约来源（已冻结，见 `docs/20-架构/音频架构.md` §5）：
 * - 事件名带冒号：`player:state`（状态变化 + 4 Hz）、`player:position`（4 Hz，仅 Playing）。
 *   点号在 Tauri 事件名里非法，不要写成 `player.state`。
 * - 命令名带点：`player.snapshot` / `player.play` / `player.pause` / `player.next` /
 *   `player.prev` / `player.seek` / `player.volume`；参数按 Tauri 约定是 camelCase
 *   （Rust 侧 `position_ms` 在前端写 `positionMs`）。
 * - 成功返回完整 `PlaybackState`；失败抛 `{ code, message }`。
 *
 * 浏览器（无 Tauri 运行时）退化为 mock：不抛错、可交互，保证 `pnpm dev` 能预览。
 * mock 边界：单曲 216s 的假曲目、250ms 位置 tick（对齐真实 4 Hz）、`next`/`prev`
 * 与真实后端一样返回 `E_UNSUPPORTED`，不模拟队列。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** 与 Rust `PlaybackStatus` 一致（serde snake_case）。 */
export type PlaybackStatus = "idle" | "playing" | "paused" | "ended" | "error";

/** 与 Rust `RepeatMode` 一致。 */
export type RepeatMode = "off" | "one" | "all";

/** 与 Rust `PlaybackMode` 一致（serde camelCase）。 */
export type PlaybackMode = {
  repeat: RepeatMode;
  shuffle: boolean;
};

/** 与 Rust `PlayerError` 一致：`{ code, message }`。 */
export type PlayerErrorInfo = {
  code: string;
  message: string;
};

/** `PlaybackState` 的前端镜像（字段与 Rust 侧 serde camelCase 一一对应）。 */
export type PlaybackState = {
  status: PlaybackStatus;
  trackId: string | null;
  positionMs: number;
  durationMs: number | null;
  volume: number;
  mode: PlaybackMode;
  error: PlayerErrorInfo | null;
};

/** 线上事件名（冒号，冻结）。 */
export const PLAYER_STATE_EVENT = "player:state";
export const PLAYER_POSITION_EVENT = "player:position";

/** 命令通道名（点号，冻结）。 */
export type PlayerCommandName =
  | "player.snapshot"
  | "player.play"
  | "player.pause"
  | "player.next"
  | "player.prev"
  | "player.seek"
  | "player.volume";

/** 命令参数；每个命令只取自己需要的字段。 */
export type PlayerCommandArgs = {
  /** `player.play`：本地文件绝对路径；省略则继续当前曲目。 */
  path?: string;
  /** `player.seek`：目标位置（毫秒）。 */
  positionMs?: number;
  /** `player.volume`：0.0–1.0。 */
  volume?: number;
};

export type PlayerBridgeHandlers = {
  /** `player:state`，或首帧 `player.snapshot`。 */
  onState?: (state: PlaybackState) => void;
  /** `player:position` 的 `positionMs`。 */
  onPosition?: (positionMs: number) => void;
  /** 首帧快照 / 订阅本身失败（不是命令失败，命令失败由调用方 catch）。 */
  onError?: (error: PlayerErrorInfo) => void;
};

export type Unsubscribe = () => void;

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

/** 把任意异常归一成 `{ code, message }`，UI 只认这个形状。 */
export function toPlayerError(error: unknown): PlayerErrorInfo {
  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error &&
    typeof (error as { code: unknown }).code === "string" &&
    typeof (error as { message: unknown }).message === "string"
  ) {
    const { code, message } = error as PlayerErrorInfo;
    return { code, message };
  }

  if (error instanceof Error) {
    return { code: "E_UNKNOWN", message: error.message };
  }

  return { code: "E_UNKNOWN", message: String(error) };
}

/**
 * 订阅播放器事件。
 *
 * 顺序按任务卡固定为「先 snapshot 再 listen」：先拿首帧，再挂监听，
 * 避免小窗挂载前的状态缺失；代价是两者之间的事件可能丢，可接受
 * （随后 4 Hz 的 `player:state` 会补齐）。
 *
 * 返回取消函数；重复调用安全。浏览器下退化为 mock，永不抛错。
 */
export function subscribe(handlers: PlayerBridgeHandlers): Unsubscribe {
  if (!isTauriRuntime()) {
    return subscribeMock(handlers);
  }

  let disposed = false;
  const unlisteners: Array<() => void> = [];

  void (async () => {
    try {
      const snapshot = await invokePlayer("player.snapshot");
      if (!disposed) {
        handlers.onState?.(snapshot);
      }
    } catch (error) {
      if (!disposed) {
        handlers.onError?.(toPlayerError(error));
      }
    }

    if (disposed) {
      return;
    }

    try {
      const offState = await listen<PlaybackState>(PLAYER_STATE_EVENT, (event) => {
        const state = normalizeState(event.payload);
        if (state && !disposed) {
          handlers.onState?.(state);
        }
      });
      if (disposed) {
        offState();
      } else {
        unlisteners.push(offState);
      }

      const offPosition = await listen<{ positionMs?: number }>(
        PLAYER_POSITION_EVENT,
        (event) => {
          const position = event.payload?.positionMs;
          if (typeof position === "number" && !disposed) {
            handlers.onPosition?.(position);
          }
        },
      );
      if (disposed) {
        offPosition();
      } else {
        unlisteners.push(offPosition);
      }
    } catch (error) {
      if (!disposed) {
        handlers.onError?.(toPlayerError(error));
      }
    }
  })();

  return () => {
    disposed = true;
    for (const off of unlisteners.splice(0)) {
      off();
    }
  };
}

/**
 * 发一条播放器命令。
 *
 * 失败统一抛 `{ code, message }`；调用方必须 catch 并展示，不许静默吞。
 */
export async function invokePlayer(
  command: PlayerCommandName,
  args?: PlayerCommandArgs,
): Promise<PlaybackState> {
  if (!isTauriRuntime()) {
    return mockInvoke(command, args);
  }

  try {
    const result = await invoke<PlaybackState>(command, args);
    const state = normalizeState(result);

    if (!state) {
      throw { code: "E_UNKNOWN", message: `${command} 返回了非法状态` } satisfies PlayerErrorInfo;
    }

    return state;
  } catch (error) {
    throw toPlayerError(error);
  }
}

/** 运行时校验事件载荷：字段可疑时丢弃而不是渲染脏数据。 */
function normalizeState(raw: unknown): PlaybackState | null {
  if (typeof raw !== "object" || raw === null) {
    return null;
  }

  const value = raw as Record<string, unknown>;
  const status = value.status;
  if (
    status !== "idle" &&
    status !== "playing" &&
    status !== "paused" &&
    status !== "ended" &&
    status !== "error"
  ) {
    return null;
  }

  const mode = (value.mode ?? {}) as Record<string, unknown>;
  const repeat = mode.repeat;
  const error = value.error as Record<string, unknown> | null | undefined;

  return {
    status,
    trackId: typeof value.trackId === "string" ? value.trackId : null,
    positionMs: toFiniteNumber(value.positionMs, 0),
    durationMs:
      typeof value.durationMs === "number" && Number.isFinite(value.durationMs)
        ? value.durationMs
        : null,
    volume: clamp01(toFiniteNumber(value.volume, 1)),
    mode: {
      repeat: repeat === "one" || repeat === "all" ? repeat : "off",
      shuffle: mode.shuffle === true,
    },
    error:
      typeof error === "object" &&
      error !== null &&
      typeof error.code === "string" &&
      typeof error.message === "string"
        ? { code: error.code, message: error.message }
        : null,
  };
}

function toFiniteNumber(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}

/* ── mock（浏览器预览） ───────────────────────────────── */

const MOCK_TRACK_ID = "C:\\Music\\Demo\\浏览器预览音轨.wav";
const MOCK_DURATION_MS = 216_000;
const MOCK_TICK_MS = 250;
const MOCK_START_VOLUME = 0.8;

type MockSession = {
  state: PlaybackState;
  handlers: Set<PlayerBridgeHandlers>;
  ticker: ReturnType<typeof setInterval> | null;
  lastTickAt: number;
};

let mockSession: MockSession | null = null;

function cloneState(state: PlaybackState): PlaybackState {
  return {
    ...state,
    mode: { ...state.mode },
    error: state.error ? { ...state.error } : null,
  };
}

function createMockState(): PlaybackState {
  return {
    status: "playing",
    trackId: MOCK_TRACK_ID,
    positionMs: 0,
    durationMs: MOCK_DURATION_MS,
    volume: MOCK_START_VOLUME,
    mode: { repeat: "off", shuffle: false },
    error: null,
  };
}

function session(): MockSession {
  if (!mockSession) {
    mockSession = {
      state: createMockState(),
      handlers: new Set(),
      ticker: null,
      lastTickAt: 0,
    };
  }

  return mockSession;
}

/** 测试与 dev 预览用：把 mock 恢复到初始状态。 */
export function resetMockPlayer(): void {
  if (mockSession?.ticker) {
    clearInterval(mockSession.ticker);
  }

  mockSession = null;
}

function subscribeMock(handlers: PlayerBridgeHandlers): Unsubscribe {
  const current = session();
  current.handlers.add(handlers);

  // mock 的快照同步派发，避免预览首帧出现空窗。
  handlers.onState?.(cloneState(current.state));
  syncTicker(current);

  return () => {
    current.handlers.delete(handlers);
    syncTicker(current);
  };
}

function syncTicker(current: MockSession): void {
  const shouldTick = current.state.status === "playing" && current.handlers.size > 0;

  if (!shouldTick) {
    if (current.ticker) {
      clearInterval(current.ticker);
      current.ticker = null;
    }
    return;
  }

  if (current.ticker) {
    return;
  }

  current.lastTickAt = Date.now();
  current.ticker = setInterval(() => {
    const now = Date.now();
    const elapsed = Math.max(0, now - current.lastTickAt);
    current.lastTickAt = now;

    const duration = current.state.durationMs ?? 0;
    const next = Math.min(duration, current.state.positionMs + elapsed);
    current.state.positionMs = next;

    for (const handler of current.handlers) {
      handler.onPosition?.(next);
    }

    if (duration > 0 && next >= duration) {
      current.state.status = "ended";
      emitMockState(current);
      syncTicker(current);
    }
  }, MOCK_TICK_MS);
}

function emitMockState(current: MockSession): void {
  const snapshot = cloneState(current.state);
  for (const handler of current.handlers) {
    handler.onState?.(snapshot);
  }
}

async function mockInvoke(
  command: PlayerCommandName,
  args?: PlayerCommandArgs,
): Promise<PlaybackState> {
  const current = session();
  const state = current.state;

  switch (command) {
    case "player.snapshot":
      break;

    case "player.play": {
      if (args?.path) {
        state.trackId = args.path;
        state.positionMs = 0;
        state.error = null;
      } else if (state.status === "ended") {
        state.positionMs = 0;
      }

      if (!state.trackId) {
        throw { code: "E_UNSUPPORTED", message: "mock 模式下没有可播放曲目" } satisfies PlayerErrorInfo;
      }

      state.status = "playing";
      break;
    }

    case "player.pause":
      if (state.status === "playing") {
        state.status = "paused";
      }
      break;

    case "player.seek": {
      const duration = state.durationMs ?? 0;
      const target = Math.min(duration, Math.max(0, args?.positionMs ?? 0));
      state.positionMs = target;
      if (state.status === "ended" && target < duration) {
        state.status = "paused";
      }
      break;
    }

    case "player.volume":
      state.volume = clamp01(args?.volume ?? state.volume);
      break;

    case "player.next":
    case "player.prev":
      // 与真实后端一致：队列未实现前固定 E_UNSUPPORTED（T-0015 才补队列）。
      throw {
        code: "E_UNSUPPORTED",
        message: "队列尚未实现（T-0015 补齐）",
      } satisfies PlayerErrorInfo;

    default:
      throw { code: "E_UNSUPPORTED", message: `未知命令 ${command}` } satisfies PlayerErrorInfo;
  }

  emitMockState(current);
  syncTicker(current);

  return cloneState(state);
}
