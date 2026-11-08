/**
 * xterm 渲染面（T-0007 §6.3 / T-0029 §6.5）。
 *
 * 只做四件事：挂载 Terminal + FitAddon、主题变化时 dispose 重建、
 * blocks 变化时 reset 后整段重写、卸载时 dispose。
 * 结构化 → ANSI 的降级逻辑全部在纯函数 `blocksToAnsi` 里。
 *
 * 为什么要在主题变化时重建：xterm 把颜色画在 canvas 上、读不到 CSS 变量，
 * 也没有运行时换主题的 API。所以监听 `<html>` 的 `data-theme` / `data-skin` /
 * inline `style`，变化后按当前计算样式重新生成 `ITheme`（T-0029 §6.5）。
 */
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef, useState } from "react";
import type { Block } from "../../../../shared/terminal-model";
import { blocksToAnsi } from "./blocks-to-ansi";
import { readXtermTheme, xtermOptionsFor } from "./xterm-theme";

/** 便于单测替换的最小终端接口。 */
export interface BlockWriter {
  reset: () => void;
  write: (data: string) => void;
}

/**
 * 复位后整段重写；`reset()` 与 `write()` 的先后顺序被测试固定。
 */
export function writeBlocks(terminal: BlockWriter, blocks: Block[]): void {
  terminal.reset();
  terminal.write(blocksToAnsi(blocks));
}

export interface XtermSurfaceProps {
  blocks: Block[];
  /** 宿主 div 的 class；尺寸由调用方决定（xterm 需要父元素有明确宽高）。 */
  className?: string;
}

export function XtermSurface({ blocks, className }: XtermSurfaceProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  /** 最近一次 blocks：主题切换重建 Terminal 后要立刻补写内容。 */
  const blocksRef = useRef<Block[]>(blocks);
  blocksRef.current = blocks;

  /**
   * 主题代次：`data-theme` / `data-skin` / inline `style` 任一变化 +1。
   *
   * 主题加载器写 inline 变量、实验室顶栏切 `data-theme` 都会让这里重建
   * Terminal，从而让 xterm 的颜色跟随当前主题（T-0029 §6.5）。
   */
  const [themeRevision, setThemeRevision] = useState(0);

  useEffect(() => {
    if (typeof document === "undefined" || typeof MutationObserver === "undefined") {
      return;
    }

    const observer = new MutationObserver(() => {
      setThemeRevision((revision) => revision + 1);
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme", "data-skin", "style"],
    });

    return () => {
      observer.disconnect();
    };
  }, []);

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) {
      return;
    }

    // 每次重建都按当前主题重新取色；读不到 CSS 变量时回落 graphite。
    const terminal = new Terminal(xtermOptionsFor(readXtermTheme()));
    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(host);

    const fit = (): void => {
      try {
        fitAddon.fit();
      } catch {
        // 容器隐藏或测试环境测量不到字符尺寸时保持默认 cols/rows，不阻断渲染。
      }
    };
    fit();
    terminalRef.current = terminal;
    // 重建后旧画布已销毁，必须立刻把现有块写回去。
    writeBlocks(terminal, blocksRef.current);

    window.addEventListener("resize", fit);
    return () => {
      window.removeEventListener("resize", fit);
      terminalRef.current = null;
      terminal.dispose();
    };
  }, [themeRevision]);

  useEffect(() => {
    const terminal = terminalRef.current;
    if (terminal === null) {
      return;
    }
    writeBlocks(terminal, blocks);
  }, [blocks]);

  return <div ref={hostRef} data-testid="xterm-surface" className={className} />;
}
