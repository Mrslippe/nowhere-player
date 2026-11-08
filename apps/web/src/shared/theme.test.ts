import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  DEFAULT_THEME,
  loadTheme,
  saveTheme,
  THEME_STORAGE_KEY,
} from "./theme";

/** 内存版 Storage：测试环境是 node，没有真 localStorage。 */
function memoryStorage(seed: Record<string, string> = {}): Storage {
  const data = new Map(Object.entries(seed));

  return {
    get length() {
      return data.size;
    },
    clear() {
      data.clear();
    },
    getItem(key: string) {
      return data.get(key) ?? null;
    },
    key(index: number) {
      return [...data.keys()][index] ?? null;
    },
    removeItem(key: string) {
      data.delete(key);
    },
    setItem(key: string, value: string) {
      data.set(key, value);
    },
  };
}

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("主题持久化", () => {
  it("没存过主题时回落 graphite", () => {
    expect(DEFAULT_THEME).toBe("graphite");
    expect(loadTheme()).toBe("graphite");
  });

  it("saveTheme 写入 np.theme，loadTheme 能原样读回", () => {
    saveTheme("amber");

    expect(THEME_STORAGE_KEY).toBe("np.theme");
    expect(globalThis.localStorage.getItem("np.theme")).toBe("amber");
    expect(loadTheme()).toBe("amber");
  });

  it("存了非法值也回落 graphite", () => {
    vi.stubGlobal("localStorage", memoryStorage({ "np.theme": "neon" }));

    expect(loadTheme()).toBe("graphite");
  });

  it("localStorage 抛错时读回落默认值、写不抛出", () => {
    vi.stubGlobal("localStorage", {
      getItem() {
        throw new Error("SecurityError");
      },
      setItem() {
        throw new Error("QuotaExceededError");
      },
    } as unknown as Storage);

    expect(loadTheme()).toBe(DEFAULT_THEME);
    expect(() => saveTheme("ice")).not.toThrow();
  });
});
