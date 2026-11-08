import { useRef, useState } from "react";
import type {
  KeyboardEvent as ReactKeyboardEvent,
  PointerEvent as ReactPointerEvent,
  WheelEvent as ReactWheelEvent,
} from "react";
import { Icon, IconButton, ProgressBar, Spinner } from "../../design";
import type { PlayerErrorInfo } from "../../shared/player-bridge";
import { coverBackground, coverInitial } from "./cover";
import { formatTime } from "./format";
import { positionFromSeekRatio, seekRatioFromPointer } from "./playback";
import type { PlayerState } from "./state";
import "./player.css";

/** 滚轮一格（≈一次 wheel 事件）的音量步长。 */
const VOLUME_STEP = 0.05;
/** 键盘方向键的跳转步长。 */
const KEY_SEEK_STEP_MS = 5000;

/**
 * 真实封面是否可见：没有 URL、或**同一个** URL 曾加载失败时回退渐变。
 * 换成新 URL（换歌 / 重新扫描）时重新给图片一次机会。
 */
export function coverImageVisible(
  coverSrc: string | null,
  failedCoverSrc: string | null,
): boolean {
  return Boolean(coverSrc) && failedCoverSrc !== coverSrc;
}

export type PlayerCardProps = {
  title: string;
  artist: string;
  album: string;
  coverSeed: string;
  /** 真实封面 URL；`null` / 未解析完成时显示确定性渐变（T-0025）。 */
  coverSrc?: string | null;
  durationMs: number;
  positionMs: number;
  state: PlayerState;
  /** 订阅到的异步错误或命令失败；非空时显示红点，悬停可看 `{code, message}`。 */
  error?: PlayerErrorInfo | null;
  /** 一次性提示（如 `next` 返回 E_UNSUPPORTED），不阻塞操作。 */
  notice?: string | null;
  /** 滚轮调音量后的短暂提示。 */
  volumeHint?: string | null;
  /** 正在拖动进度条（暂停外推由外层负责）。 */
  dragging?: boolean;
  onToggle?: () => void;
  onPrev?: () => void;
  onNext?: () => void;
  onSeekPreview?: (positionMs: number) => void;
  onSeekCommit?: (positionMs: number) => void;
  onVolumeNudge?: (delta: number) => void;
};

/**
 * 播放器小窗卡片：只负责展示与指针交互，状态与命令由外层注入。
 * 340×120 的窗口内容区由外层 .np-player-stage 固定，卡片自身撑满。
 */
export function PlayerCard({
  title,
  artist,
  album,
  coverSeed,
  coverSrc = null,
  durationMs,
  positionMs,
  state,
  error = null,
  notice = null,
  volumeHint = null,
  dragging = false,
  onToggle,
  onPrev,
  onNext,
  onSeekPreview,
  onSeekCommit,
  onVolumeNudge,
}: PlayerCardProps) {
  const [seeking, setSeeking] = useState(false);
  const [failedCoverSrc, setFailedCoverSrc] = useState<string | null>(null);
  const seekRectRef = useRef<{ left: number; width: number } | null>(null);

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
  const errorTitle = error ? `${error.code}：${error.message}` : "播放错误";
  const canSeek = !controlsDisabled && shownDuration > 0;
  const isScrubbing = seeking || dragging;
  // 同一 URL 加载失败后不再重试；换封面（URL 变化）时重新给图片一次机会。
  const showCoverImage = coverImageVisible(coverSrc, failedCoverSrc);

  const measure = (element: HTMLElement) => {
    const rect = element.getBoundingClientRect();
    const measured = { left: rect.left, width: rect.width };
    seekRectRef.current = measured;

    return measured;
  };

  const positionAt = (clientX: number, rect: { left: number; width: number }) =>
    positionFromSeekRatio(seekRatioFromPointer(clientX, rect.left, rect.width), shownDuration);

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!canSeek) {
      return;
    }

    const rect = measure(event.currentTarget);
    event.currentTarget.setPointerCapture?.(event.pointerId);
    setSeeking(true);
    onSeekPreview?.(positionAt(event.clientX, rect));
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!canSeek || !seeking) {
      return;
    }

    const rect = seekRectRef.current ?? measure(event.currentTarget);
    onSeekPreview?.(positionAt(event.clientX, rect));
  };

  const handlePointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!seeking) {
      return;
    }

    const rect = seekRectRef.current ?? measure(event.currentTarget);
    setSeeking(false);
    onSeekCommit?.(positionAt(event.clientX, rect));
  };

  const handlePointerCancel = () => {
    if (!seeking) {
      return;
    }

    // 取消时按当前展示位置提交，避免位置跳回。
    setSeeking(false);
    onSeekCommit?.(shownPosition);
  };

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (!canSeek) {
      return;
    }

    if (event.key === "ArrowLeft") {
      event.preventDefault();
      onSeekCommit?.(Math.max(0, shownPosition - KEY_SEEK_STEP_MS));
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      onSeekCommit?.(Math.min(shownDuration, shownPosition + KEY_SEEK_STEP_MS));
    } else if (event.key === "Home") {
      event.preventDefault();
      onSeekCommit?.(0);
    } else if (event.key === "End") {
      event.preventDefault();
      onSeekCommit?.(shownDuration);
    }
  };

  const handleWheel = (event: ReactWheelEvent<HTMLElement>) => {
    if (controlsDisabled || !onVolumeNudge || event.deltaY === 0) {
      return;
    }

    onVolumeNudge(event.deltaY < 0 ? VOLUME_STEP : -VOLUME_STEP);
  };

  return (
    <article
      className="np-player-card"
      data-state={state}
      data-controls={controlsVisible ? "visible" : "hidden"}
      data-seeking={isScrubbing ? "true" : "false"}
      data-tauri-drag-region="true"
      aria-busy={isLoading || undefined}
      onWheel={handleWheel}
    >
      <div
        className="np-player-card__cover"
        style={{ backgroundImage: coverBackground(coverSeed) }}
      >
        {showCoverImage ? (
          <img
            className="np-player-card__cover-image"
            src={coverSrc ?? undefined}
            alt=""
            decoding="async"
            loading="eager"
            draggable={false}
            onError={() => setFailedCoverSrc(coverSrc ?? null)}
          />
        ) : null}

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
          <IconButton
            aria-label="上一首"
            size="sm"
            disabled={controlsDisabled}
            onClick={onPrev}
          >
            <Icon name="skip-back" size={14} />
          </IconButton>
          <IconButton
            aria-label={showPlay ? "播放" : "暂停"}
            size="sm"
            disabled={controlsDisabled}
            onClick={onToggle}
          >
            <Icon name={showPlay ? "play" : "pause"} size={14} />
          </IconButton>
          <IconButton
            aria-label="下一首"
            size="sm"
            disabled={controlsDisabled}
            onClick={onNext}
          >
            <Icon name="skip-forward" size={14} />
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
          <div
            className="np-player-card__seek"
            role="slider"
            tabIndex={canSeek ? 0 : -1}
            aria-label="播放进度（左右键跳转 5 秒）"
            aria-valuemin={0}
            aria-valuemax={Math.round(shownDuration)}
            aria-valuenow={Math.round(shownPosition)}
            aria-disabled={canSeek ? undefined : true}
            onPointerDown={handlePointerDown}
            onPointerMove={handlePointerMove}
            onPointerUp={handlePointerUp}
            onPointerCancel={handlePointerCancel}
            onKeyDown={handleKeyDown}
          >
            <ProgressBar
              value={progress}
              size={2}
              indeterminate={isLoading}
              label="播放进度"
              className="np-player-card__progress"
            />
          </div>
          <span className="np-player-card__time">{time}</span>
        </div>
      </div>

      {volumeHint ? (
        <span
          className="np-player-card__volume"
          role="status"
          title={`音量 ${volumeHint}`}
        >
          音量 {volumeHint}
        </span>
      ) : null}

      {isError || error ? (
        <span
          className="np-player-card__error-dot"
          role="img"
          aria-label={error ? `播放错误：${error.code}` : "播放错误"}
          title={errorTitle}
        />
      ) : null}

      {notice ? (
        <span className="np-player-card__notice" role="status" title={notice}>
          {notice}
        </span>
      ) : null}
    </article>
  );
}
