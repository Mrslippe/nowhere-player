import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { PlayerView } from "./PlayerView";

describe("PlayerView", () => {
  it("无 Tauri 运行时下先渲染空闲态卡片，并标记 mock 模式", () => {
    const html = renderToStaticMarkup(<PlayerView />);

    expect(html).toContain('data-window="player"');
    expect(html).toContain("np-player-card");
    expect(html).toContain('data-state="idle"');
    expect(html).toContain('data-mock="true"');
    expect(html).toContain("从配置窗选择歌曲");
  });

  it("dev 下渲染「实时」+ 6 状态切换面板（含 ?state= 入口）", () => {
    const html = renderToStaticMarkup(<PlayerView />);

    expect(html).toContain('aria-label="播放器状态预览"');
    expect(html).toContain('title="实时状态"');
    expect(html).toContain("np-player-dev__toggle");
    expect(html).toContain('data-open="false"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain('aria-controls="np-player-dev-panel"');

    const states = ["idle", "loading", "playing", "paused", "error", "hover"];
    for (const state of states) {
      expect(html).toContain(`title="?state=${state}"`);
    }
  });
});
