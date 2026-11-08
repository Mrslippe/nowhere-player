import { useEffect, useState } from "react";
import { Button } from "../../design";
import fixture from "../../shared/fixtures/terminal-demo.json";
import type { Block } from "../../shared/terminal-model";
import { isBlock } from "../../shared/terminal-model.guard";
import { CommandInput } from "./components/CommandInput";
import { TerminalStage } from "./components/Stage";
import { applyTheme, parseTheme, type LabTheme } from "./components/theme";
import {
  LAB_VARIANTS,
  parseVariant,
  VARIANT_LABELS,
  type LabVariant,
} from "./components/variant";
import { DEMO_TIME, runMockCommand } from "./mock-commands";
import "./terminal.css";

const THEME_LABELS: Readonly<Record<LabTheme, string>> = {
  graphite: "graphite",
  amber: "amber",
  ice: "ice",
};

function readParam(name: string): string | null {
  if (typeof window === "undefined") {
    return null;
  }

  return new URLSearchParams(window.location.search).get(name);
}

/** 同步 URL（不刷新、不新增历史条目），便于截图与直链。 */
function writeParam(name: string, value: string): void {
  if (typeof window === "undefined") {
    return;
  }

  const url = new URL(window.location.href);
  url.searchParams.set(name, value);
  window.history.replaceState(null, "", url);
}

/** 首屏种子：shared fixture（覆盖 6 种原语）+ 两条可输入的演示命令。 */
function createInitialBlocks(): Block[] {
  const blocks: Block[] = [];

  if (isBlock(fixture)) {
    blocks.push(fixture);
  }

  blocks.push(runMockCommand("search 晴天", { now: DEMO_TIME + 1 }));
  blocks.push(runMockCommand("perm", { now: DEMO_TIME + 2 }));

  return blocks;
}

export interface TerminalLabProps {
  /** 测试/预览用：覆盖 URL 读取。 */
  initialVariant?: LabVariant;
  initialTheme?: LabTheme;
  initialBlocks?: Block[];
  /** 关闭 URL 同步与后续的 localStorage 持久化（测试用）。 */
  persist?: boolean;
}

/**
 * 终端对比实验室（T-0008）。
 *
 * 顶栏（视图 / 主题 / 清空）→ 终端区域 → 输入行 → 评分面板。
 * 两个渲染器共享同一个 `blocks` 状态，实验室里不存在第二份假数据。
 */
export function TerminalLab({
  initialVariant,
  initialTheme,
  initialBlocks,
  persist = true,
}: TerminalLabProps) {
  const [variant, setVariant] = useState<LabVariant>(
    () => initialVariant ?? parseVariant(readParam("variant")),
  );
  const [theme, setTheme] = useState<LabTheme>(
    () => initialTheme ?? parseTheme(readParam("theme")),
  );
  const [blocks, setBlocks] = useState<Block[]>(() => initialBlocks ?? createInitialBlocks());

  // 首屏从 ?theme= 进入时也要落到 <html data-theme>；切换时同步（不刷新）。
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  const selectVariant = (next: LabVariant): void => {
    setVariant(next);
    if (persist) {
      writeParam("variant", next);
    }
  };

  const selectTheme = (next: LabTheme): void => {
    setTheme(next);
    applyTheme(next);
    if (persist) {
      writeParam("theme", next);
    }
  };

  const submit = (input: string): void => {
    setBlocks((previous) => [...previous, runMockCommand(input)]);
  };

  const clear = (): void => {
    setBlocks([]);
  };

  return (
    <div className="np-lab" data-testid="terminal-lab" data-variant={variant}>
      <header className="np-lab__topbar">
        <span className="np-lab__brand">终端对比实验室</span>

        <div className="np-lab__group" role="group" aria-label="渲染视图">
          {LAB_VARIANTS.map((value) => (
            <button
              key={value}
              type="button"
              className="np-lab__chip"
              data-variant={value}
              aria-pressed={variant === value}
              onClick={() => selectVariant(value)}
            >
              {VARIANT_LABELS[value]}
            </button>
          ))}
        </div>

        <div className="np-lab__group" role="group" aria-label="主题">
          {(["graphite", "amber", "ice"] as const).map((value) => (
            <button
              key={value}
              type="button"
              className="np-lab__chip"
              data-theme={value}
              aria-pressed={theme === value}
              onClick={() => selectTheme(value)}
            >
              {THEME_LABELS[value]}
            </button>
          ))}
        </div>

        <span className="np-lab__spacer" />

        <Button variant="ghost" size="sm" onClick={clear}>
          清空
        </Button>
      </header>

      <TerminalStage variant={variant} blocks={blocks} />

      <CommandInput onSubmit={submit} />
    </div>
  );
}
