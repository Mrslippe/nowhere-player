import { useEffect } from "react";
// 必须在其它 import 之前求值：先记下 URL 里的原始 ?theme=，再做主题持久化。
import { previewSkin, previewTheme } from "./dev-preview";
import { PlayerView } from "../features/player/PlayerView";
import { TerminalView } from "../features/terminal/TerminalView";
import type { WindowLabel } from "./window-label";

/*
 * 窗口根元素挂 skin（ADR-0003 / T-0027 / T-0028）。
 * 结构变量写在 design/skin/ 下；默认 plain，dev 下 `?skin=phosphor` 可预览磷光皮肤。
 * 在首帧前落到 <html>，避免启动时先渲染默认结构再切换。
 */
if (typeof document !== "undefined") {
  if (previewSkin === "phosphor") {
    document.documentElement.dataset.skin = "phosphor";
  } else {
    document.documentElement.dataset.skin = "plain";
  }
}

/** 无路由库：同一份包按窗口 label 选择视图。 */
export function App({ label }: { label: WindowLabel }) {
  /*
   * 主题在子组件的首帧 effect 之后再落一次：终端实验室只认三套旧主题，
   * 会把 `?theme=phosphor` 解析回 graphite。父级 effect 最后执行，最终配色以 URL 为准。
   * 生产构建 previewTheme 恒为 null，不覆盖主题持久化。
   */
  useEffect(() => {
    if (previewTheme === null) {
      return;
    }

    document.documentElement.dataset.theme = previewTheme;

    // 主题持久化会把无法识别的 phosphor 回写成 graphite；写回 URL，刷新后仍是磷光。
    if (previewTheme === "phosphor") {
      const url = new URL(window.location.href);
      url.searchParams.set("theme", "phosphor");
      window.history.replaceState(null, "", url);
    }
  }, []);

  return label === "terminal" ? <TerminalView /> : <PlayerView />;
}
