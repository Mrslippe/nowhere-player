import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  invokePlayer,
  isTauriRuntime,
  PLAYER_POSITION_EVENT,
  PLAYER_STATE_EVENT,
  resetMockPlayer,
  subscribe,
  toPlayerError,
} from "./player-bridge";
import type { PlaybackState, PlayerCommandName } from "./player-bridge";

beforeEach(() => {
  resetMockPlayer();
});

afterEach(() => {
  resetMockPlayer();
  vi.useRealTimers();
});

describe("冻结的线上契约", () => {
  it("事件名是 模块:事件，不含点号（Tauri 会拒绝 IllegalEventName）", () => {
    expect(PLAYER_STATE_EVENT).toBe("player:state");
    expect(PLAYER_POSITION_EVENT).toBe("player:position");
    expect(PLAYER_STATE_EVENT).not.toContain(".");
    expect(PLAYER_POSITION_EVENT).not.toContain(".");
  });

  it("未知命令也被归一成 { code, message }，不会静默成功", async () => {
    await expect(
      invokePlayer("player.unknown" as PlayerCommandName),
    ).rejects.toMatchObject({ code: "E_UNSUPPORTED" });
  });
});

describe("player-bridge 在浏览器 / node（无 Tauri）下退化为 mock", () => {
  it("没有 Tauri 运行时", () => {
    expect(isTauriRuntime()).toBe(false);
  });

  it("subscribe 不抛错，并同步派发首帧快照", () => {
    const states: PlaybackState[] = [];
    const off = subscribe({ onState: (state) => states.push(state) });

    expect(states).toHaveLength(1);
    expect(states[0]?.status).toBe("playing");
    expect(states[0]?.trackId).toContain("浏览器预览音轨");
    off();
  });

  it("命令成功会派发新状态；取消订阅后不再收到", async () => {
    const states: PlaybackState[] = [];
    const off = subscribe({ onState: (state) => states.push(state) });

    const paused = await invokePlayer("player.pause");
    expect(paused.status).toBe("paused");
    expect(states.at(-1)?.status).toBe("paused");

    const count = states.length;
    off();

    const playing = await invokePlayer("player.play");
    expect(playing.status).toBe("playing");
    expect(states).toHaveLength(count);
  });

  it("next / prev 与真实后端一样返回 E_UNSUPPORTED", async () => {
    await expect(invokePlayer("player.next")).rejects.toMatchObject({
      code: "E_UNSUPPORTED",
    });
    await expect(invokePlayer("player.prev")).rejects.toMatchObject({
      code: "E_UNSUPPORTED",
    });
  });

  it("seek 钳制在 [0, duration]", async () => {
    expect((await invokePlayer("player.seek", { positionMs: -10 })).positionMs).toBe(0);
    expect(
      (await invokePlayer("player.seek", { positionMs: 10 })).positionMs,
    ).toBe(10);
    expect(
      (await invokePlayer("player.seek", { positionMs: 999_999 })).positionMs,
    ).toBe(216_000);
  });

  it("volume 钳制在 [0, 1]", async () => {
    expect((await invokePlayer("player.volume", { volume: 2 })).volume).toBe(1);
    expect((await invokePlayer("player.volume", { volume: -1 })).volume).toBe(0);
  });

  it("4 Hz（250ms）位置事件按真实节奏推进", async () => {
    vi.useFakeTimers();
    const positions: number[] = [];
    const off = subscribe({ onPosition: (positionMs) => positions.push(positionMs) });

    vi.advanceTimersByTime(250);
    vi.advanceTimersByTime(250);

    expect(positions).toEqual([250, 500]);
    off();
  });
});

describe("toPlayerError", () => {
  it("保留 { code, message } 原样", () => {
    expect(toPlayerError({ code: "E_DECODE", message: "坏文件" })).toEqual({
      code: "E_DECODE",
      message: "坏文件",
    });
  });

  it("Error 与字符串兜底成 E_UNKNOWN", () => {
    expect(toPlayerError(new Error("boom"))).toEqual({
      code: "E_UNKNOWN",
      message: "boom",
    });
    expect(toPlayerError("boom")).toEqual({ code: "E_UNKNOWN", message: "boom" });
  });
});
