/**
 * ↑/↓ 命令历史（T-0020 §6.2）。
 *
 * 历史存 `localStorage["np.cmd.history"]`，最新一条在末尾，上限 500 条。
 * 读写全部包 `try/catch`：隐私模式、配额满、字段被篡改都不能让输入框崩掉。
 *
 * 翻页语义：`index = -1` 表示“没在翻历史”，此时输入框里是用户正在写的草稿；
 * 第一次 `↑` 记住草稿，翻回底部时恢复它。连续重复的命令不入栈。
 */

/** localStorage key（T-0020 §6.2 冻结）。 */
export const HISTORY_KEY = "np.cmd.history";

/** 历史条数上限；超出丢最旧。 */
export const HISTORY_LIMIT = 500;

/** 历史只依赖这两个方法，测试可注入内存实现。 */
export interface HistoryStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** 历史浏览状态，`entries` 不变，只有 `index` / `draft` 变。 */
export interface HistoryCursor {
  /** 已执行的命令，最旧在前、最新在末尾。 */
  entries: readonly string[];
  /** 当前停在历史里的下标；`-1` 表示在草稿上。 */
  index: number;
  /** 开始翻历史前用户没提交的输入。 */
  draft: string;
}

/** 新建游标（停在草稿上）。 */
export function createHistoryCursor(
  entries: readonly string[] = [],
): HistoryCursor {
  return { entries, index: -1, draft: "" };
}

/**
 * 提交一条命令后的历史：去空白、去空、连续重复不入栈、超过 500 条丢最旧。
 *
 * 返回新数组（不修改入参）；未变化时也返回新数组，调用方可以无脑 `setState`。
 */
export function pushHistory(
  entries: readonly string[],
  input: string,
): string[] {
  const command = input.trim();
  const next = [...entries];

  if (command === "") {
    return next;
  }

  if (next[next.length - 1] === command) {
    return next;
  }

  next.push(command);
  return next.length > HISTORY_LIMIT
    ? next.slice(next.length - HISTORY_LIMIT)
    : next;
}

/** 取默认存储；拿不到（SSR / 隐私模式抛错）返回 `null`。 */
function defaultStorage(): HistoryStorage | null {
  try {
    return typeof globalThis.localStorage === "undefined"
      ? null
      : globalThis.localStorage;
  } catch {
    return null;
  }
}

/**
 * 读历史。任何异常（无 localStorage、JSON 损坏、类型不对）都回落为空数组。
 */
export function loadHistory(
  storage: HistoryStorage | null = defaultStorage(),
): string[] {
  try {
    const raw = storage?.getItem(HISTORY_KEY);
    if (raw === null || raw === undefined) {
      return [];
    }

    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      return [];
    }

    return parsed
      .filter(
        (entry): entry is string =>
          typeof entry === "string" && entry.trim() !== "",
      )
      .slice(-HISTORY_LIMIT);
  } catch {
    return [];
  }
}

/** 写历史；失败（隐私模式 / 配额）静默丢弃，历史只活在内存里。 */
export function saveHistory(
  entries: readonly string[],
  storage: HistoryStorage | null = defaultStorage(),
): void {
  try {
    storage?.setItem(
      HISTORY_KEY,
      JSON.stringify(entries.slice(-HISTORY_LIMIT)),
    );
  } catch {
    // 有意吞掉：持久化失败不应打断输入。
  }
}

/** `↑`：回到更旧的一条；在草稿上时先记住草稿。 */
export function historyUp(
  cursor: HistoryCursor,
  currentInput: string,
): { cursor: HistoryCursor; value: string } {
  if (cursor.entries.length === 0) {
    return { cursor, value: currentInput };
  }

  const index =
    cursor.index === -1
      ? cursor.entries.length - 1
      : Math.max(cursor.index - 1, 0);
  const draft = cursor.index === -1 ? currentInput : cursor.draft;

  return {
    cursor: { entries: cursor.entries, index, draft },
    value: cursor.entries[index],
  };
}

/** `↓`：回到更新的一条；越过最后一条恢复草稿。 */
export function historyDown(
  cursor: HistoryCursor,
  currentInput: string,
): { cursor: HistoryCursor; value: string } {
  if (cursor.index === -1) {
    return { cursor, value: currentInput };
  }

  if (cursor.index >= cursor.entries.length - 1) {
    return {
      cursor: { entries: cursor.entries, index: -1, draft: cursor.draft },
      value: cursor.draft,
    };
  }

  const index = cursor.index + 1;
  return {
    cursor: { entries: cursor.entries, index, draft: cursor.draft },
    value: cursor.entries[index],
  };
}
