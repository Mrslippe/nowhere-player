import { useEffect, useRef } from "react";
import type { Block } from "../../../shared/terminal-model";
import { BlocksView } from "../renderers/blocks/BlocksView";
import { XtermSurface } from "../renderers/xterm/XtermSurface";
import type { LabVariant } from "./variant";

function getViewport(wrapper: HTMLElement | null): HTMLElement | null {
  return wrapper?.querySelector<HTMLElement>(".xterm-viewport") ?? null;
}

function maxScroll(element: HTMLElement): number {
  return Math.max(0, element.scrollHeight - element.clientHeight);
}

function scrollRatio(element: HTMLElement): number {
  const max = maxScroll(element);

  return max > 0 ? element.scrollTop / max : 0;
}

export interface TerminalStageProps {
  variant: LabVariant;
  blocks: Block[];
  /** 实验台才显示 pane 头（A 结构化 / B xterm）；产品视图只有块流。 */
  lab?: boolean;
}

/**
 * 终端区域：按 variant 挂载 A（结构化）/ B（xterm）或左右分屏。
 *
 * 两个 pane 收到的是同一个 `blocks` 数组——实验室的“可比”前提（T-0008 §6.3）。
 * both 模式下按滚动百分比同步（§6.4）：xterm 的滚动容器是内部 `.xterm-viewport`，
 * 从宿主 div 里查；查不到就静默降级为不同步。
 */
export function TerminalStage({ variant, blocks, lab = true }: TerminalStageProps) {
  const stageRef = useRef<HTMLElement | null>(null);
  const blockScrollRef = useRef<HTMLDivElement | null>(null);
  const xtermWrapRef = useRef<HTMLDivElement | null>(null);

  // 容器尺寸变化（评分面板展开、窗口缩放）时让 xterm 重新 fit。
  // XtermSurface 只监听 window.resize（渲染器已冻结），这里从外部补一个 ResizeObserver。
  useEffect(() => {
    const stage = stageRef.current;

    if (stage === null || typeof ResizeObserver === "undefined") {
      return;
    }

    let frame = 0;
    const observer = new ResizeObserver(() => {
      if (frame !== 0) {
        cancelAnimationFrame(frame);
      }
      frame = requestAnimationFrame(() => {
        frame = 0;
        window.dispatchEvent(new Event("resize"));
      });
    });

    observer.observe(stage);

    return () => {
      observer.disconnect();
      if (frame !== 0) {
        cancelAnimationFrame(frame);
      }
    };
  }, [variant]);

  // 新块到来后贴底；切换视图回到顶部，保证首屏两边都从同一起点开始看。
  // 首次挂载不滚动：截图/直链时从历史开头阅读，而不是直接掉到末尾。
  // T-0019：同一 id 的最终块会替换 `running` 块（数量不变但内容变多），
  // 所以末尾块换了引用也要贴底，而不是只看长度。
  const previousVariant = useRef(variant);
  const previousCount = useRef(blocks.length);
  const previousLast = useRef<Block | null>(blocks[blocks.length - 1] ?? null);

  useEffect(() => {
    const scroll = blockScrollRef.current;
    const variantChanged = previousVariant.current !== variant;
    const grew = blocks.length > previousCount.current;
    const last = blocks[blocks.length - 1] ?? null;
    const lastChanged = last !== previousLast.current;

    previousVariant.current = variant;
    previousCount.current = blocks.length;
    previousLast.current = last;

    if (variantChanged) {
      if (scroll !== null) {
        scroll.scrollTop = 0;
      }
      const viewport = getViewport(xtermWrapRef.current);
      if (viewport !== null) {
        viewport.scrollTop = 0;
      }
      return;
    }

    if (!grew && !lastChanged) {
      return;
    }

    if (scroll !== null) {
      scroll.scrollTop = scroll.scrollHeight;
    }

    // xterm 的 write 是异步的，等两帧再贴底，避免读到旧的 scrollHeight。
    const scrollXtermToBottom = (): void => {
      const viewport = getViewport(xtermWrapRef.current);
      if (viewport !== null) {
        viewport.scrollTop = viewport.scrollHeight;
      }
    };
    let second = 0;
    const first = requestAnimationFrame(() => {
      scrollXtermToBottom();
      second = requestAnimationFrame(scrollXtermToBottom);
    });

    return () => {
      cancelAnimationFrame(first);
      if (second !== 0) {
        cancelAnimationFrame(second);
      }
    };
  }, [blocks, variant]);

  // both 模式：scroll 事件 + requestAnimationFrame 节流，按百分比双向同步。
  useEffect(() => {
    if (variant !== "both") {
      return;
    }

    const left = blockScrollRef.current;
    const wrapper = xtermWrapRef.current;

    if (left === null || wrapper === null) {
      return;
    }

    let right: HTMLElement | null = null;
    let frame = 0;

    const schedule = (callback: () => void): void => {
      if (frame !== 0) {
        return;
      }

      frame = requestAnimationFrame(() => {
        frame = 0;
        callback();
      });
    };

    const syncRight = (): void => {
      if (right === null) {
        return;
      }

      const target = scrollRatio(left) * maxScroll(right);
      if (Math.abs(right.scrollTop - target) < 1) {
        return;
      }

      right.scrollTop = target;
    };

    const syncLeft = (): void => {
      if (right === null) {
        return;
      }

      const target = scrollRatio(right) * maxScroll(left);
      if (Math.abs(left.scrollTop - target) < 1) {
        return;
      }

      left.scrollTop = target;
    };

    const onLeftScroll = (): void => schedule(syncRight);
    const onRightScroll = (): void => schedule(syncLeft);

    // XtermSurface 在 StrictMode 下会重建 Terminal，viewport 节点也随之替换，故用 observer 跟着换。
    const attachViewport = (): void => {
      const next = getViewport(wrapper);

      if (next === right) {
        return;
      }

      right?.removeEventListener("scroll", onRightScroll);
      right = next;
      right?.addEventListener("scroll", onRightScroll, { passive: true });
    };

    attachViewport();
    const observer =
      typeof MutationObserver === "undefined" ? null : new MutationObserver(attachViewport);
    observer?.observe(wrapper, { childList: true, subtree: true });
    left.addEventListener("scroll", onLeftScroll, { passive: true });

    return () => {
      left.removeEventListener("scroll", onLeftScroll);
      right?.removeEventListener("scroll", onRightScroll);
      observer?.disconnect();
      if (frame !== 0) {
        cancelAnimationFrame(frame);
      }
    };
  }, [variant]);

  return (
    <section
      ref={stageRef}
      className="np-lab__stage"
      data-variant={variant}
      aria-label={lab ? "终端对比区域" : "终端输出"}
    >
      {variant !== "xterm" ? (
        <div className="np-lab__pane" data-pane="blocks" data-block-count={blocks.length}>
          {lab ? (
            <div className="np-lab__pane-head">
              <span className="np-lab__pane-key">A</span>
              结构化
            </div>
          ) : null}
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
          {lab ? (
            <div className="np-lab__pane-head">
              <span className="np-lab__pane-key">B</span>
              xterm.js
            </div>
          ) : null}
          <div className="np-lab__xterm-wrap" ref={xtermWrapRef}>
            <XtermSurface blocks={blocks} className="np-lab__xterm" />
          </div>
        </div>
      ) : null}
    </section>
  );
}
