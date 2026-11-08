import { PlayerView } from "../features/player/PlayerView";
import { TerminalView } from "../features/terminal/TerminalView";
import type { WindowLabel } from "./window-label";

/** 无路由库：同一份包按窗口 label 选择视图。 */
export function App({ label }: { label: WindowLabel }) {
  return label === "terminal" ? <TerminalView /> : <PlayerView />;
}
