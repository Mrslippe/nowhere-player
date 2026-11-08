import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { MediaOutput as MediaOutputModel } from "../../../../../shared/terminal-model";
import { MediaOutput } from "./MediaOutput";

const OUTPUT: MediaOutputModel = {
  type: "media",
  kind: "cover",
  ref: "https://example.com/cover.jpg",
  size: 48,
};

describe("MediaOutput", () => {
  it("ref 是 URL 时渲染 48×48 的 <img>", () => {
    const html = renderToStaticMarkup(<MediaOutput output={OUTPUT} />);

    expect(html).toContain("<img");
    expect(html).toContain('src="https://example.com/cover.jpg"');
    expect(html).toContain('alt="封面"');
    expect(html).toContain('width="48"');
    expect(html).toContain('height="48"');
  });

  it("非 URL 的 ref 退化为确定性渐变占位", () => {
    const html = renderToStaticMarkup(
      <MediaOutput output={{ ...OUTPUT, ref: "library/album/7f3c/cover.jpg" }} />,
    );

    expect(html).not.toContain("<img");
    expect(html).toContain('role="img"');
    expect(html).toContain('aria-label="封面占位"');
    expect(html).toContain("linear-gradient");
    expect(html).toContain("var(--accent-soft)");
    expect(html).toContain("var(--bg-elev)");
  });

  it("thumb 使用缩略图文案，并采用载荷里的边长", () => {
    const html = renderToStaticMarkup(
      <MediaOutput
        output={{ ...OUTPUT, kind: "thumb", ref: "not a url", size: 64 }}
      />,
    );

    expect(html).toContain('aria-label="缩略图占位"');
    expect(html).toContain("width:64px");
    expect(html).toContain("height:64px");
  });
});
