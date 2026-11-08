import { useState } from "react";
import { Button } from "../../design";
import { PlayerCard } from "./PlayerCard";
import { mockForState } from "./mock";
import { coverSeedForTrack, previewStateFromStatus, trackTitle } from "./playback";
import {
  PLAYER_STATE_LABELS,
  PLAYER_STATES,
  readPlayerStateOverride,
} from "./state";
import type { PlayerState } from "./state";
import { usePlayer } from "./use-player";
import { useTrackCoverSrc } from "./use-track-cover";
import "./player.css";

/** dev 才允许 ?state= 钉住静态原型与状态面板；打包后小窗只跟真实状态走。 */
const IS_DEV = import.meta.env.DEV;

/** T-0013 还没有曲目元数据（`trackId` 只是本地路径），副标题只能给中性文案。 */
const LIVE_ARTIST = "本地文件";
const LIVE_ALBUM = "未知专辑";

/**
 * 播放器小窗入口：订阅真实播放状态渲染卡片。
 *
 * dev 下仍保留 `?state=`：给了合法状态就钉住 T-0004 的静态原型（含 6 状态面板），
 * 不给则和打包后一样跟随 `player:state`；浏览器（无 Tauri）由 bridge 退化为 mock。
 */
export function PlayerView() {
  const [pinned, setPinned] = useState<PlayerState | null>(() =>
    IS_DEV ? readPlayerStateOverride() : null,
  );
  const player = usePlayer({ enabled: pinned === null });
  const live = pinned === null;
  const status = player.state.status;

  const selectPinned = (next: PlayerState | null) => {
    setPinned(next);

    if (typeof window === "undefined") {
      return;
    }

    const url = new URL(window.location.href);
    if (next === null) {
      url.searchParams.delete("state");
    } else {
      url.searchParams.set("state", next);
    }
    window.history.replaceState(null, "", url);
  };

  const mock = live ? null : mockForState(pinned);
  // ended 时进度条归位（位置 0、按钮回到 ▶）。
  const livePositionMs = status === "ended" ? 0 : player.positionMs;
  // 播放状态只带文件路径；封面走 trackByPath → coverUrl（T-0025）。
  const coverSrc = useTrackCoverSrc(live ? player.state.trackId : null);

  // 静态原型（?state=）不接命令，只有实时模式才注入控件回调。
  const handlers = live
    ? {
        onToggle: player.onToggle,
        onPrev: player.onPrev,
        onNext: player.onNext,
        onSeekPreview: player.onSeekPreview,
        onSeekCommit: player.onSeekCommit,
        onVolumeNudge: player.onVolumeNudge,
        volumeHint: player.volumeHint,
        dragging: player.dragging,
      }
    : {};

  return (
    <main
      className="np-player-view"
      data-window="player"
      data-mock={player.isMock ? "true" : "false"}
      aria-label="Nowhere Player 播放器"
    >
      <div className="np-player-stage">
        <PlayerCard
          title={live ? trackTitle(player.state.trackId) : mock?.title ?? ""}
          artist={live ? LIVE_ARTIST : mock?.artist ?? ""}
          album={live ? LIVE_ALBUM : mock?.album ?? ""}
          coverSeed={
            live ? coverSeedForTrack(player.state.trackId) : mock?.coverSeed ?? ""
          }
          coverSrc={coverSrc}
          durationMs={live ? player.state.durationMs ?? 0 : mock?.durationMs ?? 0}
          positionMs={live ? livePositionMs : mock?.positionMs ?? 0}
          state={live ? previewStateFromStatus(status) : pinned}
          error={live ? player.error : null}
          notice={live ? player.notice : null}
          {...handlers}
        />
      </div>

      {IS_DEV ? (
        <nav className="np-player-switcher" aria-label="播放器状态预览">
          <Button
            size="sm"
            variant={live ? "primary" : "ghost"}
            aria-pressed={live}
            title="实时状态"
            onClick={() => selectPinned(null)}
          >
            实时
          </Button>
          {PLAYER_STATES.map((name) => (
            <Button
              key={name}
              size="sm"
              variant={name === pinned ? "primary" : "ghost"}
              aria-pressed={name === pinned}
              title={`?state=${name}`}
              onClick={() => selectPinned(name)}
            >
              {PLAYER_STATE_LABELS[name]}
            </Button>
          ))}
        </nav>
      ) : null}
    </main>
  );
}
