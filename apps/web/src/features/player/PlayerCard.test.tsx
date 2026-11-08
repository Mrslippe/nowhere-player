import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { PlayerCard, coverImageVisible } from "./PlayerCard";
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

    expect(html).toContain('data-tauri-drag-region="deep"');
    // 控件与进度条显式退出拖拽区，点按钮/拖进度不会带窗移动。
    expect(html).toContain('data-tauri-drag-region="false"');
    expect(html).toContain("Tame Impala · Currents");
    expect(html).toContain("00:42/03:36");
    expect(html).toContain('role="progressbar"');
    expect(html).toContain('aria-valuenow="19"');
  });

  it("专辑为空时副标题只显示艺人，不留分隔符（T-0026）", () => {
    const html = renderCard("playing", { album: "" });

    expect(html).toContain("Tame Impala");
    expect(html).not.toContain("Tame Impala ·");
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

  it("有真实封面 URL 时渲染 <img>，渐变仍作为加载/失败的底", () => {
    const html = renderCard("playing", {
      coverSrc: "http://asset.localhost/C%3A%5Ccover.jpg",
    });

    expect(html).toContain('class="np-player-card__cover-image"');
    expect(html).toContain('src="http://asset.localhost/C%3A%5Ccover.jpg"');
    expect(html).toContain('alt=""');
    expect(html).toContain('decoding="async"');
    expect(html).toContain('loading="eager"');
    expect(coverBackgroundOf(html)).toContain("linear-gradient(");
  });

  it("没有封面 URL 时只显示渐变，不渲染 <img>", () => {
    expect(renderCard("playing")).not.toContain(
      "np-player-card__cover-image",
    );
    expect(renderCard("playing", { coverSrc: null })).not.toContain(
      "np-player-card__cover-image",
    );
  });

  it("封面可见性：同一 URL 失败后回退，换 URL 重新尝试", () => {
    expect(coverImageVisible(null, null)).toBe(false);
    expect(coverImageVisible("", null)).toBe(false);
    expect(coverImageVisible("cover-a", null)).toBe(true);
    expect(coverImageVisible("cover-a", "cover-a")).toBe(false);
    expect(coverImageVisible("cover-b", "cover-a")).toBe(true);
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

  it("控制键使用 IconButton 的 sm 尺寸（不再靠 player.css 覆写）", () => {
    const html = renderCard("hover");

    expect(html.match(/data-size="sm"/g)).toHaveLength(3);
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

  it("异步 error 非空时显示红点，悬停提示带 {code, message}", () => {
    const html = renderCard("playing", {
      error: { code: "E_DEVICE_LOST", message: "输出设备已断开" },
    });

    expect(html).toContain("np-player-card__error-dot");
    expect(html).toContain('aria-label="播放错误：E_DEVICE_LOST"');
    expect(html).toContain(
      'title="E_DEVICE_LOST：输出设备已断开"',
    );
  });

  it("无 error 时不显示红点", () => {
    expect(renderCard("playing")).not.toContain(
      "np-player-card__error-dot",
    );
  });

  it("notice 一次性提示渲染为 role=status，不阻塞操作", () => {
    const html = renderCard("paused", {
      notice: "下一首失败 · E_UNSUPPORTED",
    });

    expect(html).toContain('role="status"');
    expect(html).toContain("np-player-card__notice");
    expect(html).toContain("下一首失败 · E_UNSUPPORTED");
  });

  it("进度条外包一层可拖动的 role=slider 热区，aria 值跟随位置", () => {
    const html = renderCard("playing", { positionMs: 6000, durationMs: 12_000 });

    expect(html).toContain('class="np-player-card__seek"');
    expect(html).toContain('role="slider"');
    expect(html).toContain('aria-valuemax="12000"');
    expect(html).toContain('aria-valuenow="6000"');
    expect(html).toContain('tabindex="0"');
  });

  it("进度比例写成 --np-progress，0 / 结束 / 越界都钳在 0–1", () => {
    expect(
      renderCard("playing", { positionMs: 0, durationMs: 216_000 }),
    ).toContain("--np-progress:0");
    expect(
      renderCard("playing", { positionMs: 216_000, durationMs: 216_000 }),
    ).toContain("--np-progress:1");
    expect(
      renderCard("playing", { positionMs: 999_999, durationMs: 216_000 }),
    ).toContain("--np-progress:1");
    expect(
      renderCard("playing", { positionMs: -1, durationMs: 216_000 }),
    ).toContain("--np-progress:0");
  });

  it("没有时长时热区不可聚焦、不可拖动", () => {
    const html = renderCard("idle", { durationMs: 0 });

    expect(html).toContain('aria-disabled="true"');
    expect(html).toContain('tabindex="-1"');
  });

  it("滚轮调音量后的提示只在传入时渲染", () => {
    expect(renderCard("playing")).not.toContain("np-player-card__volume");
    expect(renderCard("playing", { volumeHint: "85%" })).toContain(
      "音量 85%",
    );
  });

  it("音量浮层：静音不带“音量”前缀，淡出由 data-fading 驱动", () => {
    const quiet = renderCard("playing", { volumeHint: "静音" });
    expect(quiet).toContain("np-player-card__volume");
    expect(quiet).toContain('title="静音"');
    expect(quiet).not.toContain("音量 静音");

    const fading = renderCard("playing", {
      volumeHint: "42%",
      volumeHintFading: true,
    });
    expect(fading).toContain('data-fading="true"');
    expect(fading).toContain("音量 42%");
  });
});
