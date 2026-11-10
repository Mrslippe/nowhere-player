import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { Completion } from "../../shared/command-bridge";
import { CommandInput, focusBarClass } from "./components/CommandInput";
import {
  COMPLETION_KIND_LABELS,
  CompletionPopup,
} from "./components/CompletionPopup";

function completion(count: number): Completion {
  return {
    replaceStart: 0,
    replaceEnd: 0,
    items: Array.from({ length: count }, (_, index) => ({
      text: `play${index}`,
      kind: index % 2 === 0 ? "command" : "suggestion",
      summary: `第 ${index} 条`,
    })),
  };
}

describe("补全弹层渲染（T-0020 §6.3）", () => {
  it("无候选时不渲染任何节点", () => {
    const html = renderToStaticMarkup(
      <CompletionPopup completion={completion(0)} activeIndex={0} />,
    );

    expect(html).toBe("");
  });

  it("最多渲染 5 条，并标出高亮项与类型标签", () => {
    const html = renderToStaticMarkup(
      <CompletionPopup completion={completion(12)} activeIndex={1} />,
    );

    const rows = html.match(/class="np-lab__completion-item/g) ?? [];
    expect(rows).toHaveLength(5);
    expect(html).toContain('data-count="5"');
    expect(html).toContain('role="listbox"');
    expect(html).toContain('aria-label="命令补全"');
    // 第 1 条高亮，第 0 条不高亮。
    expect(html).toContain('data-active="true"');
    expect(html).toContain(COMPLETION_KIND_LABELS.command);
    expect(html).toContain(COMPLETION_KIND_LABELS.suggestion);
    expect(html).toContain("第 4 条");
    // 第 9 条以后被截掉。
    expect(html).not.toContain("play5");
  });
});

describe("输入行焦点条 class（T-0050 B）", () => {
  it("focus / blur 切换只翻 is-focused class", () => {
    expect(focusBarClass(false)).toBe("");
    expect(focusBarClass(true)).toBe("is-focused");
  });
});

describe("焦点条形状（T-0050 B 修正）", () => {
  it("下层卡用 2px 偏移 + 同圆角 + clip-path，不再 scaleX", async () => {
    const fs = (await import("node:fs" as string)) as {
      readFileSync: (path: URL, encoding: "utf8") => string;
    };
    const css = fs.readFileSync(new URL("./terminal.css", import.meta.url), "utf8");

    expect(css).toContain("inset: auto 0 -2px 0");
    expect(css).toContain("height: 14px");
    expect(css).toContain("border-bottom-left-radius: var(--np-radius-control)");
    expect(css).toContain("border-bottom-right-radius: var(--np-radius-control)");
    expect(css).toContain("clip-path: inset(0 100% 0 0)");
    expect(css).toContain("clip-path: inset(0 0 0 0)");
    expect(css).not.toContain("scaleX(");
    expect(css).toContain("np-completion-enter");
    expect(css).toContain("np-completion-update");
    expect(css).toContain("translateY(4px)");
    expect(css).toContain("animation: none");
  });
});

describe("输入行结构", () => {
  it("默认不弹层：aria-expanded=false 且没有弹层节点", () => {
    const html = renderToStaticMarkup(
      <CommandInput onSubmit={() => undefined} persistHistory={false} />,
    );

    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain('aria-autocomplete="list"');
    expect(html).not.toContain("completion-popup");
    expect(html).toContain("执行命令");
  });

  it("注入历史不影响首屏渲染（历史不占可视区域）", () => {
    const html = renderToStaticMarkup(
      <CommandInput
        onSubmit={() => undefined}
        persistHistory={false}
        initialHistory={["play 1", "queue"]}
      />,
    );

    expect(html).not.toContain("completion-popup");
    expect(html).toContain('aria-expanded="false"');
  });
});
