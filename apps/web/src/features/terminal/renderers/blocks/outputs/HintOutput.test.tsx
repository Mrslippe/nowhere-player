import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { HintOutput as HintOutputModel } from "../../../../../shared/terminal-model";
import { HintOutput } from "./HintOutput";

const OUTPUT: HintOutputModel = {
  type: "hint",
  text: "did you mean 'play'?",
  suggestion: "按 Tab 采纳",
};

describe("HintOutput", () => {
  it("渲染提示文本，并用 accent 标出建议", () => {
    const html = renderToStaticMarkup(<HintOutput output={OUTPUT} />);

    expect(html).toContain("did you mean");
    expect(html).toContain("play");
    expect(html).toContain("np-blocks-hint__suggestion");
    expect(html).toContain("按 Tab 采纳");
  });

  it("没有 suggestion 时只渲染提示文本", () => {
    const html = renderToStaticMarkup(
      <HintOutput output={{ ...OUTPUT, suggestion: null }} />,
    );

    expect(html).not.toContain("np-blocks-hint__suggestion");
    expect(html).toContain("did you mean");
  });
});
