import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { PlayerView } from "./PlayerView";

describe("PlayerView", () => {
  it("默认渲染播放态卡片，并带上 player 窗口标识", () => {
    const html = renderToStaticMarkup(<PlayerView />);

    expect(html).toContain('data-window="player"');
    expect(html).toContain("np-player-card");
    expect(html).toContain('data-state="playing"');
  });

  it("dev 下渲染 6 状态切换面板（含 ?state= 入口）", () => {
    const html = renderToStaticMarkup(<PlayerView />);

    expect(html).toContain('aria-label="播放器状态预览"');

    const states = ["idle", "loading", "playing", "paused", "error", "hover"];
    for (const state of states) {
      expect(html).toContain(`title="?state=${state}"`);
    }
  });
});
