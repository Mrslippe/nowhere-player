import { Icon, IconButton, ProgressBar, Spinner } from "../../design";
import { coverBackground, coverInitial } from "./cover";
import { formatTime } from "./format";
import type { PlayerState } from "./state";
import "./player.css";

export type PlayerCardProps = {
  title: string;
  artist: string;
  album: string;
  coverSeed: string;
  durationMs: number;
  positionMs: number;
  state: PlayerState;
};

/**
 * 播放器小窗卡片：纯展示组件，不读 URL、不发请求。
 * 340×120 的窗口内容区由外层 .np-player-stage 固定，卡片自身撑满。
 */
export function PlayerCard({
  title,
  artist,
  album,
  coverSeed,
  durationMs,
  positionMs,
  state,
}: PlayerCardProps) {
  const isIdle = state === "idle";
  const isLoading = state === "loading";
  const isError = state === "error";
  const showPlay = state !== "playing" && state !== "hover";
  const controlsDisabled = isIdle || isLoading;
  const controlsVisible = state === "hover";

  // 空闲态没有当前曲目，即使调用方还留着上一首的进度也按 0 展示。
  const shownPosition = isIdle ? 0 : positionMs;
  const shownDuration = isIdle ? 0 : durationMs;
  const ratio = shownDuration > 0 ? shownPosition / shownDuration : 0;
  const progress = Number.isFinite(ratio)
    ? Math.min(1, Math.max(0, ratio))
    : 0;
  const displayTitle = isError ? "播放失败" : isIdle ? "未在播放" : title;
  const subtitle = isIdle ? "从配置窗选择歌曲" : `${artist} · ${album}`;
  const time = `${formatTime(shownPosition)}/${formatTime(shownDuration)}`;

  return (
    <article
      className="np-player-card"
      data-state={state}
      data-controls={controlsVisible ? "visible" : "hidden"}
      data-tauri-drag-region="true"
      aria-busy={isLoading || undefined}
    >
      <div
        className="np-player-card__cover"
        style={{ backgroundImage: coverBackground(coverSeed) }}
      >
        {isLoading ? (
          <div className="np-player-card__loading">
            <Spinner size={16} label="正在加载曲目" />
          </div>
        ) : (
          <span className="np-player-card__initial" aria-hidden="true">
            {coverInitial(album)}
          </span>
        )}

        <div className="np-player-card__controls">
          <IconButton aria-label="上一首" disabled={controlsDisabled}>
            <Icon name="skip-back" size={16} />
          </IconButton>
          <IconButton
            aria-label={showPlay ? "播放" : "暂停"}
            disabled={controlsDisabled}
          >
            <Icon name={showPlay ? "play" : "pause"} size={16} />
          </IconButton>
          <IconButton aria-label="下一首" disabled={controlsDisabled}>
            <Icon name="skip-forward" size={16} />
          </IconButton>
        </div>
      </div>

      <div className="np-player-card__body">
        <h1 className="np-player-card__title" title={displayTitle}>
          {displayTitle}
        </h1>
        <p className="np-player-card__subtitle" title={subtitle}>
          {subtitle}
        </p>

        <div className="np-player-card__progress-row">
          <ProgressBar
            value={progress}
            size={2}
            indeterminate={isLoading}
            label="播放进度"
            className="np-player-card__progress"
          />
          <span className="np-player-card__time">{time}</span>
        </div>
      </div>

      {isError ? (
        <span
          className="np-player-card__error-dot"
          role="img"
          aria-label="播放错误"
        />
      ) : null}
    </article>
  );
}
