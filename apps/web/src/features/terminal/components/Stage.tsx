import { useEffect, useRef } from "react";
import type { Block } from "../../../shared/terminal-model";
import { BlocksView } from "../renderers/blocks/BlocksView";
import { XtermSurface } from "../renderers/xterm/XtermSurface";
import type { LabVariant } from "./variant";

function getViewport(wrapper: HTMLElement | null): HTMLElement | null {
  return wrapper?.querySelector<HTMLElement>(".xterm-viewport") ?? null;
}

export interface TerminalStageProps {
  variant: LabVariant;
  blocks: Block[];
}

/**
 * 终端区域：按 variant 挂载 A（结构化）/ B（xterm）或左右分屏。
 *
 * 两个 pane 收到的是同一个 `blocks` 数组——实验室的“可比”前提（T-0008 §6.3）。
 * 滚动同步与 xterm fit 的容器监听在后续提交接入（§6.4）。
 */
export function TerminalStage({ variant, blocks }: TerminalStageProps) {
  const blockScrollRef = useRef<HTMLDivElement | null>(null);
  const xtermWrapRef = useRef<HTMLDivElement | null>(null);

  // 新块到来后贴底；xterm 的 viewport 在组件挂载后才存在，故走下一帧。
  useEffect(() => {
    const scroll = blockScrollRef.current;
    if (scroll !== null) {
      scroll.scrollTop = scroll.scrollHeight;
    }

    const frame = requestAnimationFrame(() => {
      const viewport = getViewport(xtermWrapRef.current);
      if (viewport !== null) {
        viewport.scrollTop = viewport.scrollHeight;
      }
    });

    return () => cancelAnimationFrame(frame);
  }, [blocks, variant]);

  return (
    <section className="np-lab__stage" data-variant={variant} aria-label="终端对比区域">
      {variant !== "xterm" ? (
        <div className="np-lab__pane" data-pane="blocks" data-block-count={blocks.length}>
          <div className="np-lab__pane-head">
            <span className="np-lab__pane-key">A</span>
            结构化
          </div>
          <div
            className="np-lab__scroll"
            ref={blockScrollRef}
            tabIndex={0}
            aria-label="结构化终端输出"
          >
            <BlocksView blocks={blocks} />
          </div>
        </div>
      ) : null}

      {variant !== "blocks" ? (
        <div className="np-lab__pane" data-pane="xterm" data-block-count={blocks.length}>
          <div className="np-lab__pane-head">
            <span className="np-lab__pane-key">B</span>
            xterm.js
          </div>
          <div className="np-lab__xterm-wrap" ref={xtermWrapRef}>
            <XtermSurface blocks={blocks} className="np-lab__xterm" />
          </div>
        </div>
      ) : null}
    </section>
  );
}
