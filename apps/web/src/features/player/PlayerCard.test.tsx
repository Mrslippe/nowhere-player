import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { PlayerCard } from "./PlayerCard";
import type { PlayerCardProps } from "./PlayerCard";
import { PLAYER_STATES } from "./state";
import type { PlayerState } from "./state";

const BASE_PROPS: PlayerCardProps = {
  title: "The Less I Know the Better (Slowed + Reverb)",
  artist: "Tame Impala",
  album: "Currents",
  coverSeed: "currents",
  durationMs: 216_000,
  positionMs: 42_000,
  state: "playing",
};

function renderCard(
  state: PlayerState,
  overrides: Partial<PlayerCardProps> = {},
): string {
  return renderToStaticMarkup(
    <PlayerCard {...BASE_PROPS} state={state} {...overrides} />,
  );
}

function coverBackgroundOf(html: string): string {
  const match = /background-image:([^";]+)/.exec(html);

  if (!match) {
    throw new Error("卡片缺少封面渐变");
  }

  return match[1];
}

describe("PlayerCard", () => {
  it("6 种状态都能独立渲染，并带 data-state 与卡片根类名", () => {
    for (const state of PLAYER_STATES) {
      const html = renderCard(state);

      expect(html).toContain("np-player-card");
      expect(html).toContain(`data-state="${state}"`);
    }
  });

  it("渲染拖拽区、副标题、进度与时间格式", () => {
    const html = renderCard("playing");

    expect(html).toContain('data-tauri-drag-region="true"');
    expect(html).toContain("Tame Impala · Currents");
    expect(html).toContain("00:42/03:36");
    expect(html).toContain('role="progressbar"');
    expect(html).toContain('aria-valuenow="19"');
  });

  it("同一 coverSeed 两次渲染得到相同渐变，不同 seed 不同", () => {
    const first = coverBackgroundOf(renderCard("playing"));
    const second = coverBackgroundOf(renderCard("playing"));
    const other = coverBackgroundOf(
      renderCard("playing", { coverSeed: "yehuimei", album: "叶惠美" }),
    );

    expect(first).toBe(second);
    expect(first).toContain("linear-gradient(");
    expect(other).not.toBe(first);
  });

  it("idle：标题位显示未在播放", () => {
    const html = renderCard("idle");

    expect(html).toContain("未在播放");
    expect(html).toContain("从配置窗选择歌曲");
    expect(html).toContain("00:00/00:00");
  });

  it("loading：封面位 Spinner + 进度条不确定态", () => {
    const html = renderCard("loading");

    expect(html).toContain("np-player-card__loading");
    expect(html).toContain("np-spinner");
    expect(html).toContain('aria-label="正在加载曲目"');
    expect(html).toContain('data-indeterminate="true"');
    expect(html).toContain('aria-busy="true"');
  });

  it("playing：正常显示暂停键，控制键默认隐藏", () => {
    const html = renderCard("playing");

    expect(html).toContain('aria-label="暂停"');
    expect(html).toContain('data-controls="hidden"');
  });

  it("paused：播放键为 ▶", () => {
    const html = renderCard("paused");

    expect(html).toContain('aria-label="播放"');
    expect(html).not.toContain('aria-label="暂停"');
  });

  it("error：标题位显示播放失败 + 右上角红点", () => {
    const html = renderCard("error");

    expect(html).toContain("播放失败");
    expect(html).toContain("np-player-card__error-dot");
    expect(html).toContain('aria-label="播放错误"');
  });

  it("hover：强制显示控制键", () => {
    const html = renderCard("hover");

    expect(html).toContain('data-controls="visible"');
    expect(html).toContain('aria-label="暂停"');
  });

  it("超长标题完整保留在 DOM 中（CSS 负责单行省略）", () => {
    const html = renderCard("playing");

    expect(html).toContain(BASE_PROPS.title);
    expect(html).toContain(`title="${BASE_PROPS.title}"`);
  });
});
