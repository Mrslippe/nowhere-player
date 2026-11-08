import { useState } from "react";
import { Button } from "../../design";
import { PlayerCard } from "./PlayerCard";
import { mockForState } from "./mock";
import { PLAYER_STATE_LABELS, PLAYER_STATES, readPlayerState } from "./state";
import type { PlayerState } from "./state";
import "./player.css";

/** dev 才允许 ?state= 覆盖与状态面板；打包后小窗从 idle 起步。 */
const IS_DEV = import.meta.env.DEV;

/**
 * 播放器小窗入口：读 `?state=` 决定演示状态，渲染纯展示的 PlayerCard。
 * 状态面板只在 dev 出现，点击后同步 URL，便于一键切换与截图。
 */
export function PlayerView() {
  const [state, setState] = useState<PlayerState>(() =>
    IS_DEV ? readPlayerState() : "idle",
  );
  const track = mockForState(state);

  const selectState = (next: PlayerState) => {
    setState(next);

    if (typeof window === "undefined") {
      return;
    }

    const url = new URL(window.location.href);
    url.searchParams.set("state", next);
    window.history.replaceState(null, "", url);
  };

  return (
    <main
      className="np-player-view"
      data-window="player"
      aria-label="Nowhere Player 播放器"
    >
      <div className="np-player-stage">
        <PlayerCard {...track} state={state} />
      </div>

      {IS_DEV ? (
        <nav className="np-player-switcher" aria-label="播放器状态预览">
          {PLAYER_STATES.map((name) => (
            <Button
              key={name}
              size="sm"
              variant={name === state ? "primary" : "ghost"}
              aria-pressed={name === state}
              title={`?state=${name}`}
              onClick={() => selectState(name)}
            >
              {PLAYER_STATE_LABELS[name]}
            </Button>
          ))}
        </nav>
      ) : null}
    </main>
  );
}
