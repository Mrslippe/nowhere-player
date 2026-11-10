/**
 * Tab 补全的纯逻辑（T-0020 §6.1 / §6.3）。
 *
 * 后端给的区间是**字符**偏移；`<input>` 的 `selectionStart` 是 UTF-16 偏移。
 * 中文这类 BMP 字符两者相同，但 emoji 之类的代理对会差 1，所以这里统一换算，
 * 不在组件里手写 `slice`。
 *
 * 弹层交互也做成纯函数：`decideKey` 按「弹层是否打开」决定按键归属
 * （弹层打开时 ↑/↓ 归弹层，关闭时归历史），组件只负责更新状态。
 */
import type { Completion, CompletionItem } from "../../shared/command-bridge";

/** 内联补全最多显示的候选条数（T-0043 §2C）。 */
export const POPUP_LIMIT = 5;

/** 输入变化后等待多久再请求候选；卡要求 ≤80ms。 */
export const COMPLETION_DEBOUNCE_MS = 80;

/** 打开的补全弹层：候选与当前高亮下标（相对可见列表）。 */
export interface CompletionPopupState {
  completion: Completion;
  activeIndex: number;
}

/** 字符（码点）个数。 */
export function charLength(text: string): number {
  return [...text].length;
}

/** UTF-16 偏移 → 字符偏移（越界钳制；代理对中间按前一个码点算）。 */
export function toCharOffset(text: string, utf16Offset: number): number {
  const limit = Math.max(0, Math.min(utf16Offset, text.length));
  let utf16Index = 0;
  let chars = 0;

  for (const character of text) {
    if (utf16Index + character.length > limit) {
      break;
    }
    utf16Index += character.length;
    chars += 1;
  }

  return chars;
}

/** 字符偏移 → UTF-16 偏移（越界钳制）。 */
export function fromCharOffset(text: string, charOffset: number): number {
  let utf16Index = 0;
  let chars = 0;

  for (const character of text) {
    if (chars >= charOffset) {
      break;
    }
    utf16Index += character.length;
    chars += 1;
  }

  return utf16Index;
}

/** 只保留弹层可见的候选（最多 {@link POPUP_LIMIT} 条）。 */
export function visibleItems(completion: Completion): CompletionItem[] {
  return completion.items.slice(0, POPUP_LIMIT);
}

/**
 * 收到补全结果 → 弹层状态。
 *
 * 无候选返回 `null`：不弹层、不改变输入（T-0020 §6.3）。
 */
export function openCompletion(
  completion: Completion,
): CompletionPopupState | null {
  const items = visibleItems(completion);
  if (items.length === 0) {
    return null;
  }

  return {
    completion: { ...completion, items },
    activeIndex: 0,
  };
}

/** 高亮上/下移，循环（`delta` 为 ±1 或任意整数）。 */
export function moveCompletion(
  state: CompletionPopupState,
  delta: number,
): CompletionPopupState {
  const count = state.completion.items.length;
  if (count === 0) {
    return state;
  }

  return {
    ...state,
    activeIndex: (((state.activeIndex + delta) % count) + count) % count,
  };
}

/**
 * 命令名位置的可补全前缀：仅第一个词、光标在末尾、中间没有空白。
 *
 * 参数位置（含空格）返回 `null` → 不出候选（T-0043 §2C）。
 */
export function commandPrefix(input: string, caret: number): string | null {
  if (caret !== input.length || input.length === 0) {
    return null;
  }
  if (/\s/.test(input)) {
    return null;
  }
  return input;
}

/**
 * 实时候选是否该请求：历史导航期间一律不请求；其余仍要求命令名位置。
 *
 * 历史翻完后 `historyNavigating` 保持 true，直到用户再输入字符（change 里清掉），
 * 所以不会因为 value 变化自动弹候选。
 */
export function shouldRequestRealtimeCompletion(
  input: string,
  caret: number,
  historyNavigating: boolean,
): boolean {
  return !historyNavigating && commandPrefix(input, caret) !== null;
}

/** 候选弹层的过渡类型（T-0051 E）。 */
export type CompletionMotion = "none" | "enter" | "update";

function completionSignature(state: CompletionPopupState): string {
  return [
    state.completion.replaceStart,
    state.completion.replaceEnd,
    ...state.completion.items.map(
      (item) => `${item.kind}\u0000${item.text}\u0000${item.summary}`,
    ),
  ].join("\u0001");
}

/**
 * 只在“可见 → 可见且内容变了”时给 update；首次出现给 enter；
 * 内容相同（连续输入但候选集合没变）不重播。
 */
export function completionMotion(
  previous: CompletionPopupState | null,
  next: CompletionPopupState | null,
): CompletionMotion {
  if (next === null) {
    return "none";
  }
  if (previous === null) {
    return "enter";
  }
  return completionSignature(previous) === completionSignature(next)
    ? "none"
    : "update";
}

/** 当前高亮的候选；空列表为 `null`。 */
export function activeItem(
  state: CompletionPopupState,
): CompletionItem | null {
  return state.completion.items[state.activeIndex] ?? null;
}

/**
 * 采纳一条候选：只替换 `replaceStart..replaceEnd` 这一段 token。
 *
 * 命令 / 子命令 / 建议后补一个空格，方便接着按 Tab；枚举值与序号不补。
 * 若 token 后面已经有空白（光标在行中间），不再重复补空格。
 *
 * 返回的 `caret` 是 **UTF-16** 偏移，可直接给 `setSelectionRange`。
 */
export function applyCompletion(
  input: string,
  completion: Completion,
  item: CompletionItem,
): { value: string; caret: number } {
  const start = fromCharOffset(input, Math.max(0, completion.replaceStart));
  const end = Math.max(
    start,
    fromCharOffset(input, Math.max(0, completion.replaceEnd)),
  );
  const rest = input.slice(end);
  const needsSpace =
    item.kind !== "enum" &&
    item.kind !== "dynamic" &&
    !/^\s/.test(rest);
  const inserted = needsSpace ? `${item.text} ` : item.text;

  return {
    value: `${input.slice(0, start)}${inserted}${rest}`,
    caret: start + inserted.length,
  };
}

/** 组件识别的按键（已排除 IME 组合输入）。 */
export type InputKey = "tab" | "arrow-up" | "arrow-down" | "enter" | "escape";

/** 一次按键的决策结果；组件据此更新状态与光标，不再自己判断归属。 */
export interface KeyDecision {
  /** 按键之后的弹层状态（`null` = 关闭）。 */
  popup: CompletionPopupState | null;
  /** 需要请求一次补全（Tab 且弹层未打开）。 */
  requestCompletion: boolean;
  /** 历史翻页方向：-1 上一条、+1 下一条、0 不动。 */
  historyStep: -1 | 0 | 1;
  /** 已采纳的输入与光标（UTF-16）；`null` = 不采纳。 */
  adopt: { value: string; caret: number } | null;
  /** 弹层关闭时按 Enter：交给表单提交。 */
  submit: boolean;
}

const NO_OP: KeyDecision = {
  popup: null,
  requestCompletion: false,
  historyStep: 0,
  adopt: null,
  submit: false,
};

/**
 * 按键归属（T-0020 §11）：
 *
 * - 历史导航中：`↑/↓` 只翻历史，哪怕还留着一份旧弹层；
 * - 弹层打开且不在历史导航：`↑/↓` 移动高亮；
 * - 弹层关闭：`↑/↓` 翻历史；
 * - `Tab` / `Enter` / `Esc` 行为不随历史导航改变。
 */
export function decideKey(
  key: InputKey,
  input: string,
  popup: CompletionPopupState | null,
  historyNavigating = false,
): KeyDecision {
  switch (key) {
    case "tab": {
      if (popup === null) {
        return { ...NO_OP, popup: null, requestCompletion: true };
      }
      const item = popup.completion.items[0] ?? null;
      return {
        ...NO_OP,
        popup: { ...popup, activeIndex: 0 },
        adopt: item === null ? null : applyCompletion(input, popup.completion, item),
      };
    }

    case "arrow-up": {
      if (historyNavigating || popup === null) {
        return { ...NO_OP, historyStep: -1 };
      }
      return { ...NO_OP, popup: moveCompletion(popup, -1) };
    }

    case "arrow-down": {
      if (historyNavigating || popup === null) {
        return { ...NO_OP, historyStep: 1 };
      }
      return { ...NO_OP, popup: moveCompletion(popup, 1) };
    }

    case "enter":
      // Enter 永远执行当前输入；即使弹层打开也不采纳候选（T-0043 §2C）。
      return { ...NO_OP, popup: null, submit: true };

    case "escape":
      return { ...NO_OP, popup: null };
  }
}

/** DOM `KeyboardEvent.key` → {@link InputKey}；不认识的返回 `null`。 */
export function toInputKey(key: string): InputKey | null {
  switch (key) {
    case "Tab":
      return "tab";
    case "ArrowUp":
      return "arrow-up";
    case "ArrowDown":
      return "arrow-down";
    case "Enter":
      return "enter";
    case "Escape":
      return "escape";
    default:
      return null;
  }
}
