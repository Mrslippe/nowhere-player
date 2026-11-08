/**
 * xterm 渲染面（T-0007 §6.3）。
 *
 * 只做三件事：挂载 Terminal + FitAddon、blocks 变化时 reset 后整段重写、卸载时 dispose。
 * 结构化 → ANSI 的降级逻辑全部在纯函数 `blocksToAnsi` 里。
 */
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import type { Block } from "../../../../shared/terminal-model";
import { blocksToAnsi } from "./blocks-to-ansi";
import { XTERM_OPTIONS } from "./xterm-theme";

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

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) {
      return;
    }

    const terminal = new Terminal(XTERM_OPTIONS);
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

    window.addEventListener("resize", fit);
    return () => {
      window.removeEventListener("resize", fit);
      terminalRef.current = null;
      terminal.dispose();
    };
  }, []);

  useEffect(() => {
    const terminal = terminalRef.current;
    if (terminal === null) {
      return;
    }
    writeBlocks(terminal, blocks);
  }, [blocks]);

  return <div ref={hostRef} data-testid="xterm-surface" className={className} />;
}
