import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ErrorOutput as ErrorOutputModel } from "../../../../../shared/terminal-model";
import { ErrorOutput } from "./ErrorOutput";

const OUTPUT: ErrorOutputModel = {
  type: "error",
  code: "E_LIB_PERMISSION",
  message: "无法读取目录 ~/Music/Private",
  hint: "检查文件权限，或输入 scan --skip-private",
  // legacy 字段：UI 必须忽略它。
  actions: ["重试", "跳过该目录"],
};

const noop = () => {};

describe("ErrorOutput", () => {
  it("渲染错误码/消息/hint，以及块级可执行按钮（原生 button + title）", () => {
    const html = renderToStaticMarkup(
      <ErrorOutput
        output={OUTPUT}
        actions={[
          { label: "重试", input: "perm" },
          { label: "只用本地搜索", input: "search 晴天 --local" },
        ]}
        onAction={noop}
      />,
    );

    expect(html).toContain('data-code="E_LIB_PERMISSION"');
    expect(html).toContain("E_LIB_PERMISSION");
    expect(html).toContain("无法读取目录 ~/Music/Private");
    expect(html).toContain("检查文件权限");
    expect(html).toContain("np-blocks-error__actions");
    expect(html).toContain('type="button"');
    expect(html).toContain('title="perm"');
    expect(html).toContain('title="search 晴天 --local"');
    expect(html).toContain("np-btn--primary");
    expect(html).toContain("np-btn--ghost");
    expect(html).toContain("重试");
    expect(html).toContain("只用本地搜索");
    // legacy Output::Error.actions 不能被渲染成假按钮。
    expect(html).not.toContain("跳过该目录");
    expect(html).not.toContain("tabindex=\"-1\"");
  });

  it("没有 hint / 块级 actions 时不渲染对应区域；legacy actions 也不渲染", () => {
    const html = renderToStaticMarkup(
      <ErrorOutput output={{ ...OUTPUT, hint: null }} onAction={noop} />,
    );

    expect(html).not.toContain("np-blocks-error__hint");
    expect(html).not.toContain("np-blocks-error__actions");
    expect(html).not.toContain("跳过该目录");
  });

  it("没有执行回调时按钮保留但 disabled，不伪装成可执行", () => {
    const html = renderToStaticMarkup(
      <ErrorOutput
        output={OUTPUT}
        actions={[{ label: "重试", input: "perm" }]}
      />,
    );

    expect(html).toContain("disabled");
    expect(html).toContain('title="perm"');
  });
});
