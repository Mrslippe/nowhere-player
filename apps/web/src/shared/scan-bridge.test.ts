import { describe, expect, it, vi } from "vitest";

import {
  TERMINAL_BLOCK_EVENT,
  createScanBridge,
  readScanEpoch,
} from "./scan-bridge";
import type { ScanTransport, Unsubscribe } from "./scan-bridge";

function fakeTransport(): {
  transport: ScanTransport;
  emit: (payload: unknown) => void;
  unlisten: ReturnType<typeof vi.fn>;
} {
  let handler: ((payload: unknown) => void) | null = null;
  const unlisten = vi.fn();
  const transport: ScanTransport = {
    listen: async (next) => {
      handler = next;

      return unlisten as Unsubscribe;
    },
  };

  return {
    transport,
    emit: (payload) => handler?.(payload),
    unlisten,
  };
}

describe("scanBridge", () => {
  it("事件名与 Rust 侧一致（冒号，不是点号）", () => {
    expect(TERMINAL_BLOCK_EVENT).toBe("terminal:block");
    expect(TERMINAL_BLOCK_EVENT).not.toContain(".");
  });

  it("readScanEpoch 只接受正整数", () => {
    expect(readScanEpoch({ scanEpoch: 1 })).toBe(1);
    expect(readScanEpoch({ scanEpoch: 42 })).toBe(42);

    for (const payload of [
      null,
      undefined,
      {},
      { scanEpoch: 0 },
      { scanEpoch: -1 },
      { scanEpoch: 1.5 },
      { scanEpoch: Number.NaN },
      { scanEpoch: "1" },
    ]) {
      expect(readScanEpoch(payload)).toBeNull();
    }
  });

  it("只回调新代次；重复与回退的事件被丢弃", () => {
    const { transport, emit } = fakeTransport();
    const seen: number[] = [];
    const bridge = createScanBridge(transport);

    bridge.subscribe((epoch) => seen.push(epoch));

    emit({ scanEpoch: 1 });
    emit({ scanEpoch: 1 });
    emit({ scanEpoch: 0 });
    emit({ scanEpoch: 2 });

    expect(seen).toEqual([1, 2]);
  });

  it("取消订阅后不再回调；listen 晚于取消时立刻解绑", async () => {
    const { transport, emit, unlisten } = fakeTransport();
    const bridge = createScanBridge(transport);
    const seen: number[] = [];
    const off = bridge.subscribe((epoch) => seen.push(epoch));

    off();
    emit({ scanEpoch: 1 });
    // fake listen 已在 subscribe 时完成，取消应调用 unlisten。
    await Promise.resolve();

    expect(seen).toEqual([]);
    expect(unlisten).toHaveBeenCalledTimes(1);
  });
});
