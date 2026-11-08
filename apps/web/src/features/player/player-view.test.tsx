import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { PlayerView } from "./PlayerView";

describe("PlayerView", () => {
  it("渲染不报错，并带上 player 窗口标识", () => {
    const html = renderToStaticMarkup(<PlayerView />);

    expect(html).toContain('data-window="player"');
    expect(html).toContain("Nowhere Player");
  });
});
