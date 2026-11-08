import { PlayerCard } from "./PlayerCard";
import { MOCK_TRACKS } from "./mock";
import "./player.css";

/** 播放器小窗：M0 原型阶段固定渲染播放态，6 状态切换见后续增量。 */
export function PlayerView() {
  return (
    <main
      className="np-player-view"
      data-window="player"
      aria-label="Nowhere Player 播放器"
    >
      <div className="np-player-stage">
        <PlayerCard {...MOCK_TRACKS[0]} state="playing" />
      </div>
    </main>
  );
}
