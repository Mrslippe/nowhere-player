import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  createVolumeHint,
  type VolumeHintVisual,
  VOLUME_HINT_FADE_MS,
  VOLUME_HINT_MS,
} from "./volume-hint";

function setup(): {
  states: Array<VolumeHintVisual | null>;
  controller: ReturnType<typeof createVolumeHint>;
} {
  const states: Array<VolumeHintVisual | null> = [];
  const controller = createVolumeHint((state) => {
    states.push(state);
  });

  return { states, controller };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("音量浮层计时（T-0032）", () => {
  it("显示 → 到时淡出 → 再到时清空", () => {
    const { states, controller } = setup();

    controller.show("42%");
    expect(states).toEqual([{ text: "42%", fading: false }]);

    // 淡出前 1ms：仍在正常显示。
    vi.advanceTimersByTime(VOLUME_HINT_MS - VOLUME_HINT_FADE_MS - 1);
    expect(states).toHaveLength(1);

    vi.advanceTimersByTime(1);
    expect(states.at(-1)).toEqual({ text: "42%", fading: true });

    vi.advanceTimersByTime(VOLUME_HINT_FADE_MS);
    expect(states.at(-1)).toBeNull();
  });

  it("连续滚动重置计时：不提前淡出，也不叠加", () => {
    const { states, controller } = setup();

    controller.show("10%");
    vi.advanceTimersByTime(800);

    // 第二次滚动：从 800ms 起重新计时。
    controller.show("20%");
    expect(states.at(-1)).toEqual({ text: "20%", fading: false });

    // 第一次的 1020ms 淡出点（t=1020）已被取消。
    vi.advanceTimersByTime(300);
    expect(states.at(-1)).toEqual({ text: "20%", fading: false });

    // 第二次的淡出点：800 + (1200 - 180) = 1820。
    vi.advanceTimersByTime(VOLUME_HINT_MS - VOLUME_HINT_FADE_MS - 300);
    expect(states.at(-1)).toEqual({ text: "20%", fading: true });

    // 800 + 1200 = 2000 清空。
    vi.advanceTimersByTime(VOLUME_HINT_FADE_MS);
    expect(states.at(-1)).toBeNull();
    expect(states.filter((state) => state === null)).toHaveLength(1);
  });

  it("静音文案原样透传", () => {
    const { states, controller } = setup();

    controller.show("静音");
    expect(states.at(-1)).toEqual({ text: "静音", fading: false });
  });

  it("dispose 取消全部计时，不再回调", () => {
    const { states, controller } = setup();

    controller.show("30%");
    controller.dispose();
    vi.advanceTimersByTime(VOLUME_HINT_MS * 2);

    expect(states).toEqual([{ text: "30%", fading: false }]);
  });
});
