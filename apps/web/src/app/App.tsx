import { PlayerView } from "../features/player/PlayerView";
import { TerminalView } from "../features/terminal/TerminalView";
import type { WindowLabel } from "./window-label";

/*
 * 窗口根元素挂 skin（ADR-0003 / T-0027）。
 * 结构变量写在 design/skin/ 下；T-0027 只提供 plain，且必须与拆分前逐像素一致。
 * 在首帧前落到 <html>，避免启动时先渲染默认结构再切换。
 */
if (typeof document !== "undefined") {
  document.documentElement.dataset.skin = "plain";
}

/** 无路由库：同一份包按窗口 label 选择视图。 */
export function App({ label }: { label: WindowLabel }) {
  return label === "terminal" ? <TerminalView /> : <PlayerView />;
}
