import { Icon, IconButton, ProgressBar } from "../../design";
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
  const ratio = durationMs > 0 ? positionMs / durationMs : 0;
  const progress = Number.isFinite(ratio)
    ? Math.min(1, Math.max(0, ratio))
    : 0;
  const controlsVisible = state === "hover";
  const subtitle = `${artist} · ${album}`;
  const time = `${formatTime(positionMs)}/${formatTime(durationMs)}`;

  return (
    <article
      className="np-player-card"
      data-state={state}
      data-controls={controlsVisible ? "visible" : "hidden"}
      data-tauri-drag-region="true"
    >
      <div
        className="np-player-card__cover"
        style={{ backgroundImage: coverBackground(coverSeed) }}
      >
        <span className="np-player-card__initial" aria-hidden="true">
          {coverInitial(album)}
        </span>

        <div className="np-player-card__controls">
          <IconButton aria-label="上一首">
            <Icon name="skip-back" size={16} />
          </IconButton>
          <IconButton aria-label="暂停">
            <Icon name="pause" size={16} />
          </IconButton>
          <IconButton aria-label="下一首">
            <Icon name="skip-forward" size={16} />
          </IconButton>
        </div>
      </div>

      <div className="np-player-card__body">
        <h1 className="np-player-card__title" title={title}>
          {title}
        </h1>
        <p className="np-player-card__subtitle" title={subtitle}>
          {subtitle}
        </p>

        <div className="np-player-card__progress-row">
          <ProgressBar
            value={progress}
            size={2}
            label="播放进度"
            className="np-player-card__progress"
          />
          <span className="np-player-card__time">{time}</span>
        </div>
      </div>
    </article>
  );
}
