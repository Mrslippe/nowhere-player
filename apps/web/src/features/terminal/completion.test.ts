import { describe, expect, it } from "vitest";
import type { Completion, CompletionItem } from "../../shared/command-bridge";
import {
  POPUP_LIMIT,
  activeItem,
  applyCompletion,
  COMPLETION_DEBOUNCE_MS,
  charLength,
  commandPrefix,
  decideKey,
  fromCharOffset,
  shouldRequestRealtimeCompletion,
  moveCompletion,
  openCompletion,
  toCharOffset,
  toInputKey,
} from "./completion";

function item(
  text: string,
  kind: CompletionItem["kind"] = "command",
  summary = "",
): CompletionItem {
  return { text, kind, summary };
}

function completion(
  items: CompletionItem[],
  replaceStart = 0,
  replaceEnd = 0,
): Completion {
  return { replaceStart, replaceEnd, items };
}

describe("字符偏移换算（T-0020 §6.1）", () => {
  it("charLength 按码点计数", () => {
    expect(charLength("")).toBe(0);
    expect(charLength("play")).toBe(4);
    expect(charLength("中文")).toBe(2);
    expect(charLength("🎵")).toBe(1);
  });

  it("中文/BMP 字符的 UTF-16 与字符偏移一致", () => {
    expect(toCharOffset("中文 pl", 3)).toBe(3);
    expect(fromCharOffset("中文 pl", 3)).toBe(3);
  });

  it("emoji 的 UTF-16 偏移与字符偏移相差 1", () => {
    // `🎵` 是一个码点、两个 UTF-16 单元。
    expect(toCharOffset("🎵pl", 2)).toBe(1);
    expect(toCharOffset("🎵pl", 3)).toBe(2);
    expect(fromCharOffset("🎵pl", 1)).toBe(2);
    expect(fromCharOffset("🎵pl", 3)).toBe(4);
  });

  it("越界偏移被钳制", () => {
    expect(toCharOffset("pl", 99)).toBe(2);
    expect(toCharOffset("pl", -5)).toBe(0);
    expect(fromCharOffset("pl", 99)).toBe(2);
  });
});

describe("弹层状态", () => {
  it("无候选不弹层", () => {
    expect(openCompletion(completion([]))).toBeNull();
  });

  it("最多显示 5 条，初始高亮第一条", () => {
    const many = Array.from({ length: 12 }, (_, index) =>
      item(`cmd${index}`),
    );
    const popup = openCompletion(completion(many));

    expect(popup).not.toBeNull();
    expect(popup?.completion.items).toHaveLength(POPUP_LIMIT);
    expect(popup?.activeIndex).toBe(0);
    expect(activeItem(popup!)).toEqual(item("cmd0"));
  });

  it("方向键高亮循环（首尾相接）", () => {
    const popup = openCompletion(
      completion([item("a"), item("b"), item("c")]),
    )!;

    expect(moveCompletion(popup, -1).activeIndex).toBe(2);
    expect(moveCompletion(popup, 1).activeIndex).toBe(1);
    expect(moveCompletion(moveCompletion(popup, 1), 1).activeIndex).toBe(2);
    expect(moveCompletion(popup, 3).activeIndex).toBe(0);
  });
});

describe("采纳候选（只替换当前 token）", () => {
  it("命令后补空格，中文前缀按字符区间替换", () => {
    // 输入 `中文 pl`：token `pl` 是字符 3..5（BMP 下与 UTF-16 相同）。
    const result = applyCompletion(
      "中文 pl",
      completion([], 3, 5),
      item("play"),
    );

    expect(result.value).toBe("中文 play ");
    expect(result.caret).toBe(8);
  });

  it("emoji 前缀：字符区间换算成 UTF-16 后再替换", () => {
    // `🎵pl`：token `pl` 是字符 1..3，对应 UTF-16 2..4。
    const result = applyCompletion(
      "🎵pl",
      completion([], 1, 3),
      item("play"),
    );

    expect(result.value).toBe("🎵play ");
    expect(result.caret).toBe(7);
  });

  it("token 后已有空白时不重复补空格", () => {
    const result = applyCompletion(
      "pl 1",
      completion([], 0, 2),
      item("play"),
    );

    expect(result.value).toBe("play 1");
    expect(result.caret).toBe(4);
  });

  it("枚举值与序号不补空格", () => {
    const enumResult = applyCompletion(
      "mode repeat o",
      completion([], 12, 13),
      item("off", "enum"),
    );
    const dynamicResult = applyCompletion(
      "play ",
      completion([], 5, 5),
      item("2", "dynamic"),
    );

    expect(enumResult.value).toBe("mode repeat off");
    expect(enumResult.caret).toBe(15);
    expect(dynamicResult.value).toBe("play 2");
    expect(dynamicResult.caret).toBe(6);
  });

  it("区间越界时钳到输入长度，不抛错", () => {
    const result = applyCompletion(
      "pl",
      completion([], 0, 99),
      item("play"),
    );

    expect(result.value).toBe("play ");
    expect(result.caret).toBe(5);
  });
});

describe("命令名位置（T-0043 §2C）", () => {
  it("只在第一个词、光标在末尾时出候选", () => {
    expect(commandPrefix("pl", 2)).toBe("pl");
    expect(commandPrefix("", 0)).toBeNull();
    expect(commandPrefix("play ", 5)).toBeNull();
    expect(commandPrefix("play 1", 6)).toBeNull();
    expect(commandPrefix("pl", 1)).toBeNull();
    expect(commandPrefix("中", 1)).toBe("中");
  });

  it("debounce 不超过 80ms", () => {
    expect(COMPLETION_DEBOUNCE_MS).toBeLessThanOrEqual(80);
  });
});

describe("历史导航期间不触发候选（T-0046 A）", () => {
  it("historyNavigating=true 时即使命令名位置也不请求", () => {
    expect(shouldRequestRealtimeCompletion("pl", 2, true)).toBe(false);
  });

  it("historyNavigating=false 时命令名位置正常请求", () => {
    expect(shouldRequestRealtimeCompletion("pl", 2, false)).toBe(true);
  });

  it("参数位置仍然不请求", () => {
    expect(shouldRequestRealtimeCompletion("play 1", 6, false)).toBe(false);
  });

  it("空输入不请求", () => {
    expect(shouldRequestRealtimeCompletion("", 0, false)).toBe(false);
  });
});

describe("按键归属（T-0020 §11）", () => {
  it("弹层关闭时 Tab 请求补全，不改变输入", () => {
    const decision = decideKey("tab", "pl", null);

    expect(decision.requestCompletion).toBe(true);
    expect(decision.popup).toBeNull();
    expect(decision.historyStep).toBe(0);
    expect(decision.adopt).toBeNull();
  });

  it("弹层打开时 Tab 采纳第一条且不关闭列表", () => {
    const popup = openCompletion(
      completion([item("a"), item("b")]),
    )!;

    const decision = decideKey("tab", "pl", popup);

    expect(decision.requestCompletion).toBe(false);
    expect(decision.popup?.activeIndex).toBe(0);
    expect(decision.adopt?.value).toBe("a pl");
  });

  it("Tab 采纳第一条，而不是当前高亮", () => {
    const popup = openCompletion(
      completion([item("play"), item("pause")], 0, 2),
    )!;
    const moved = moveCompletion(popup, 1);

    const decision = decideKey("tab", "pl", moved);

    expect(decision.adopt?.value).toBe("play ");
    expect(decision.popup?.activeIndex).toBe(0);
  });

  it("弹层打开时 ↑/↓ 归弹层，不翻历史", () => {
    const popup = openCompletion(
      completion([item("a"), item("b"), item("c")]),
    )!;

    const up = decideKey("arrow-up", "", popup);
    const down = decideKey("arrow-down", "", popup);

    expect(up.historyStep).toBe(0);
    expect(up.popup?.activeIndex).toBe(2);
    expect(down.historyStep).toBe(0);
    expect(down.popup?.activeIndex).toBe(1);
  });

  it("弹层关闭时 ↑/↓ 归历史", () => {
    expect(decideKey("arrow-up", "", null).historyStep).toBe(-1);
    expect(decideKey("arrow-down", "", null).historyStep).toBe(1);
  });

  it("弹层打开时 Enter 执行当前输入、不采纳候选", () => {
    const popup = openCompletion(
      completion([item("a"), item("b")], 0, 1),
    )!;
    const decision = decideKey("enter", "a", popup);

    expect(decision.submit).toBe(true);
    expect(decision.popup).toBeNull();
    expect(decision.adopt).toBeNull();
  });

  it("弹层关闭时 Enter 才是执行", () => {
    const decision = decideKey("enter", "help", null);

    expect(decision.submit).toBe(true);
    expect(decision.adopt).toBeNull();
  });

  it("Esc 关闭弹层但不改输入、不执行", () => {
    const popup = openCompletion(completion([item("a")]))!;
    const decision = decideKey("escape", "a", popup);

    expect(decision.popup).toBeNull();
    expect(decision.adopt).toBeNull();
    expect(decision.submit).toBe(false);
  });

  it("DOM key 映射只认补全/历史相关按键", () => {
    expect(toInputKey("Tab")).toBe("tab");
    expect(toInputKey("ArrowUp")).toBe("arrow-up");
    expect(toInputKey("ArrowDown")).toBe("arrow-down");
    expect(toInputKey("Enter")).toBe("enter");
    expect(toInputKey("Escape")).toBe("escape");
    expect(toInputKey("a")).toBeNull();
    expect(toInputKey("Ctrl")).toBeNull();
  });
});
