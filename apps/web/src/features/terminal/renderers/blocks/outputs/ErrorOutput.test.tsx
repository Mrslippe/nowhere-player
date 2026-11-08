import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ErrorOutput as ErrorOutputModel } from "../../../../../shared/terminal-model";
import { ErrorOutput } from "./ErrorOutput";

const OUTPUT: ErrorOutputModel = {
  type: "error",
  code: "E_LIB_PERMISSION",
  message: "无法读取目录 ~/Music/Private",
  hint: "检查文件权限，或输入 scan --skip-private",
  actions: ["重试", "跳过该目录"],
};

describe("ErrorOutput", () => {
  it("渲染错误码、消息、hint 与动作按钮（仅展示）", () => {
    const html = renderToStaticMarkup(<ErrorOutput output={OUTPUT} />);

    expect(html).toContain('data-code="E_LIB_PERMISSION"');
    expect(html).toContain("E_LIB_PERMISSION");
    expect(html).toContain("无法读取目录 ~/Music/Private");
    expect(html).toContain("检查文件权限");
    expect(html).toContain("np-blocks-error__actions");
    expect(html).toContain("np-btn--ghost");
    expect(html).toContain("重试");
    expect(html).toContain("跳过该目录");
  });

  it("没有 hint / actions 时不渲染对应区域", () => {
    const html = renderToStaticMarkup(
      <ErrorOutput output={{ ...OUTPUT, hint: null, actions: null }} />,
    );

    expect(html).not.toContain("np-blocks-error__hint");
    expect(html).not.toContain("np-blocks-error__actions");
  });
});
