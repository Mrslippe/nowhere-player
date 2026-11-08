import { describe, expect, it } from "vitest";
import {
  HISTORY_KEY,
  HISTORY_LIMIT,
  createHistoryCursor,
  historyDown,
  historyUp,
  loadHistory,
  pushHistory,
  saveHistory,
  type HistoryStorage,
} from "./history";

function memoryStorage(initial: string | null = null): HistoryStorage {
  let value = initial;
  return {
    getItem() {
      return value;
    },
    setItem(_key, next) {
      value = next;
    },
  };
}

function readRaw(storage: HistoryStorage): string | null {
  return storage.getItem(HISTORY_KEY);
}

describe("历史入栈（T-0020 §6.2）", () => {
  it("key 与上限是契约值", () => {
    expect(HISTORY_KEY).toBe("np.cmd.history");
    expect(HISTORY_LIMIT).toBe(500);
  });

  it("去掉首尾空白后才入栈，纯空白不入栈", () => {
    expect(pushHistory([], "  play 1  ")).toEqual(["play 1"]);
    expect(pushHistory([], "   ")).toEqual([]);
    expect(pushHistory(["help"], "")).toEqual(["help"]);
  });

  it("连续重复不入栈", () => {
    const first = pushHistory([], "play 1");
    const second = pushHistory(first, "play 1");

    expect(second).toEqual(["play 1"]);
  });

  it("不相邻的重复命令仍然入栈", () => {
    const entries = pushHistory(pushHistory(["play 1"], "help"), "play 1");

    expect(entries).toEqual(["play 1", "help", "play 1"]);
  });

  it("超过 500 条丢最旧", () => {
    let entries: string[] = [];
    for (let index = 0; index < HISTORY_LIMIT + 5; index += 1) {
      entries = pushHistory(entries, `cmd ${index}`);
    }

    expect(entries).toHaveLength(HISTORY_LIMIT);
    expect(entries[0]).toBe("cmd 5");
    expect(entries[entries.length - 1]).toBe(`cmd ${HISTORY_LIMIT + 4}`);
  });
});

describe("历史持久化（localStorage）", () => {
  it("save → load 往返", () => {
    const storage = memoryStorage();
    saveHistory(["play 1", "queue"], storage);

    expect(loadHistory(storage)).toEqual(["play 1", "queue"]);
  });

  it("损坏的 JSON / 非数组 / 非字符串项都回落（不抛错）", () => {
    expect(loadHistory(memoryStorage("{"))).toEqual([]);
    expect(loadHistory(memoryStorage('"play"'))).toEqual([]);
    expect(loadHistory(memoryStorage('["play", 3, null, "  "]'))).toEqual([
      "play",
    ]);
    expect(loadHistory(memoryStorage())).toEqual([]);
  });

  it("读取抛错（隐私模式）时回落为空数组", () => {
    const storage: HistoryStorage = {
      getItem() {
        throw new Error("SecurityError");
      },
      setItem() {
        throw new Error("SecurityError");
      },
    };

    expect(loadHistory(storage)).toEqual([]);
    expect(() => saveHistory(["play"], storage)).not.toThrow();
  });

  it("没有 localStorage（SSR / 浏览器预览）时读写都不抛错", () => {
    expect(() => saveHistory(["play"], null)).not.toThrow();
    expect(loadHistory(null)).toEqual([]);
  });

  it("落盘时也只保留最近 500 条", () => {
    const storage = memoryStorage();
    const entries = Array.from(
      { length: HISTORY_LIMIT + 10 },
      (_, index) => `cmd ${index}`,
    );

    saveHistory(entries, storage);
    const stored: unknown = JSON.parse(readRaw(storage) ?? "[]");

    expect(Array.isArray(stored)).toBe(true);
    expect((stored as string[]).length).toBe(HISTORY_LIMIT);
    expect((stored as string[])[0]).toBe("cmd 10");
  });
});

describe("↑/↓ 翻历史", () => {
  const entries = ["play 1", "queue", "help"];

  it("↑ 从最新往回翻，并记住未提交的草稿", () => {
    let cursor = createHistoryCursor(entries);
    const first = historyUp(cursor, "vol 5");
    cursor = first.cursor;

    expect(first.value).toBe("help");
    expect(cursor.index).toBe(2);
    expect(cursor.draft).toBe("vol 5");

    const second = historyUp(cursor, first.value);
    cursor = second.cursor;
    expect(second.value).toBe("queue");

    const third = historyUp(cursor, second.value);
    expect(third.value).toBe("play 1");
    // 已在最旧一条，继续 ↑ 停住。
    expect(historyUp(third.cursor, third.value).value).toBe("play 1");
  });

  it("↓ 往前翻，越过最新一条恢复草稿", () => {
    let cursor = createHistoryCursor(entries);
    cursor = historyUp(cursor, "vol 5").cursor;
    cursor = historyUp(cursor, "help").cursor;
    cursor = historyUp(cursor, "queue").cursor;

    const down = historyDown(cursor, "play 1");
    cursor = down.cursor;
    expect(down.value).toBe("queue");

    const last = historyDown(cursor, down.value);
    cursor = last.cursor;
    expect(last.value).toBe("help");
    expect(cursor.index).toBe(2);

    const restored = historyDown(cursor, last.value);
    expect(restored.value).toBe("vol 5");
    expect(restored.cursor.index).toBe(-1);
  });

  it("空历史或已在草稿上时 ↑ 不改变输入", () => {
    expect(historyUp(createHistoryCursor([]), "typing").value).toBe("typing");
    expect(historyDown(createHistoryCursor(entries), "typing").value).toBe(
      "typing",
    );
  });
});
