/**
 * 媒体库扫描完成事件桥（T-0033）。
 *
 * 扫描完成的信号来自**已有的** `terminal:block` 事件：`library scan` 的块
 * 状态变成 `done` / `cancelled` / `error` 时即视为完成（见 `isScanFinishedBlock`）。
 * 只读结构化字段，不解析任何文本。
 *
 * 这个桥只做订阅与 epoch 过滤，不碰封面缓存；播放器 feature 收到事件后
 * 清 `path → Track` 并重查当前曲目（见 `features/player/use-cover-refresh.ts`）。
 * 浏览器预览没有 Tauri，订阅退化为 no-op。
 */
import { listen } from "@tauri-apps/api/event";

import { isTauriRuntime } from "./player-bridge";

/** 终端块事件名（Rust：`EVENT_TERMINAL_BLOCK`）。 */
export const TERMINAL_BLOCK_EVENT = "terminal:block";

/**
 * 从 `terminal:block` 载荷判断"扫描结束"。
 *
 * 为什么不加一个专用事件：Rust 侧加事件会让`nowhere-desktop`的单测二进制
 * 加载失败（0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND，可复现，与代码内容无关），
 * 排查成本高于收益。这里改用已有的**结构化**块字段（`input` / `status`），
 * 不解析任何文本。
 */
export function isScanFinishedBlock(payload: unknown): boolean {
  const block = payload as { input?: unknown; status?: unknown } | null;
  if (!block || typeof block.input !== "string" || typeof block.status !== "string") {
    return false;
  }

  const isScan = block.input.trim().startsWith("library scan");

  return isScan && ["done", "cancelled", "error"].includes(block.status);
}

export type ScanFinishedPayload = {
  /** 递增计数，不是时间戳；只接受正整数。 */
  scanEpoch: number;
};

export type Unsubscribe = () => void;

/** 可注入的监听端口，单测不碰 Tauri。 */
export type ScanTransport = {
  listen: (handler: (payload: unknown) => void) => Promise<Unsubscribe>;
};

export type ScanBridge = {
  /** 订阅新扫描代次；返回取消函数，重复调用安全。 */
  subscribe: (handler: (scanEpoch: number) => void) => Unsubscribe;
};

/** 从事件 payload 读取合法的 scanEpoch；形状不对回 null。 */
export function readScanEpoch(payload: unknown): number | null {
  if (typeof payload !== "object" || payload === null) {
    return null;
  }

  const epoch = (payload as { scanEpoch?: unknown }).scanEpoch;

  return typeof epoch === "number" && Number.isSafeInteger(epoch) && epoch > 0
    ? epoch
    : null;
}

/**
 * 造一个扫描事件桥。
 *
 * 每个订阅各自记录已见 epoch：同一事件重复到达、或旧事件乱序到达时不再
 * 回调；首次订阅只关心此后的扫描，不补发历史事件。
 */
export function createScanBridge(transport: ScanTransport): ScanBridge {
  return {
    subscribe(handler) {
      let active = true;
      let unlisten: Unsubscribe | null = null;
      let lastEpoch = 0;

      void transport
        .listen((payload) => {
          if (!active) {
            return;
          }

          const epoch = readScanEpoch(payload);
          if (epoch === null || epoch <= lastEpoch) {
            return;
          }

          lastEpoch = epoch;
          handler(epoch);
        })
        .then((off) => {
          if (active) {
            unlisten = off;
          } else {
            // 订阅建立前就取消：立刻解绑，避免泄漏。
            off();
          }
        })
        .catch(() => {
          // 扫描刷新是后台增强；订阅失败不应让播放器报错或阻塞播放。
        });

      return () => {
        active = false;
        unlisten?.();
        unlisten = null;
      };
    },
  };
}

let epochCounter = 0;

const tauriTransport: ScanTransport = {
  listen: (handler) => {
    if (!isTauriRuntime()) {
      return Promise.resolve(() => {});
    }

    return listen<unknown>(TERMINAL_BLOCK_EVENT, (event) => {
      if (isScanFinishedBlock(event.payload)) {
        epochCounter += 1;
        handler({ scanEpoch: epochCounter });
      }
    });
  },
};

/** 全应用共用的事件桥（生产只挂一次 Tauri 监听）。 */
export const scanBridge: ScanBridge = createScanBridge(tauriTransport);
