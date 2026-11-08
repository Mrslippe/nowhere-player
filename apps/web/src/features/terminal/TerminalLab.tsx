import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "../../design";
import { createCommandBridge, upsertBlock } from "../../shared/command-bridge";
import fixture from "../../shared/fixtures/terminal-demo.json";
import type { Block } from "../../shared/terminal-model";
import { isBlock } from "../../shared/terminal-model.guard";
import { CommandInput } from "./components/CommandInput";
import {
  buildExportPayload,
  collectResourceEntries,
  copyToClipboard,
} from "./components/export";
import { ScorePanel } from "./components/ScorePanel";
import {
  DEFAULT_SCORES,
  loadScores,
  saveScores,
  type ComparisonScores,
  type DimensionId,
  type ScoreSide,
} from "./components/scoring";
import { TerminalStage } from "./components/Stage";
import {
  applyTheme,
  LAB_THEMES,
  parseTheme,
  type LabTheme,
} from "./components/theme";
import {
  LAB_VARIANTS,
  parseVariant,
  VARIANT_LABELS,
  type LabVariant,
} from "./components/variant";
import { DEMO_TIME, runMockCommand } from "./mock-commands";
import { createBlockId, createWelcomeBlock, submitInput } from "./terminal-session";
import "./terminal.css";

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

/**
 * 首屏种子：欢迎块必在。
 *
 * 默认 `blocks` 是真实链路，不塞假命令输出；实验室视图（`?variant=xterm|both`）
 * 额外带上 shared fixture（覆盖 6 种原语）与两条演示命令，便于对比两套渲染器。
 */
function createInitialBlocks(variant: LabVariant): Block[] {
  const blocks: Block[] = [createWelcomeBlock()];

  if (variant === "blocks") {
    return blocks;
  }

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
  initialScores?: ComparisonScores;
  /** 关闭 URL 同步与 localStorage 持久化（测试用）。 */
  persist?: boolean;
}

/**
 * 终端视图（T-0008 实验室 → T-0019 接真实事件流）。
 *
 * 顶栏（视图 / 主题 / 清空）→ 终端区域 → 输入行 → 评分面板。
 * 两个渲染器共享同一个 `blocks` 状态，实验室里不存在第二份假数据。
 *
 * 真实链路：输入框 → `cmd.exec` → `terminal:block` 事件 → `upsertBlock` 覆盖渲染。
 * 浏览器（无 Tauri）退化为 mock，保证 `pnpm dev` 可预览。
 */
export function TerminalLab({
  initialVariant,
  initialTheme,
  initialBlocks,
  initialScores,
  persist = true,
}: TerminalLabProps) {
  // mock 兜底只在浏览器生效；真实运行时 bridge 忽略 fallback。
  // `runMockCommand` 在 features/，由这里注入，`shared/` 不反向依赖（项目结构规范 §5）。
  const bridge = useMemo(
    () =>
      createCommandBridge({
        fallback: (input, blockId) => runMockCommand(input, { id: blockId }),
      }),
    [],
  );

  const [variant, setVariant] = useState<LabVariant>(
    () => initialVariant ?? parseVariant(readParam("variant")),
  );
  const [theme, setTheme] = useState<LabTheme>(
    () => initialTheme ?? parseTheme(readParam("theme")),
  );
  // 首屏块只在首次渲染构造；`variant` 此时就是初始视图（之后切换视图不重置块）。
  const [blocks, setBlocks] = useState<Block[]>(
    () => initialBlocks ?? createInitialBlocks(variant),
  );
  const [scores, setScores] = useState<ComparisonScores>(
    () =>
      initialScores ??
      (persist ? (loadScores() ?? DEFAULT_SCORES) : DEFAULT_SCORES),
  );
  const [collapsed, setCollapsed] = useState(
    // 终端窗默认 880×560：展开评分表会把终端挤到只剩几行，矮窗先折叠。
    () => typeof window !== "undefined" && window.innerHeight < 720,
  );
  const [feedback, setFeedback] = useState<string | null>(null);
  const feedbackTimer = useRef<number | null>(null);

  // 订阅必须在挂载时就建立：`cmd.exec` 在返回回执前就 emit，
  // 「await exec 之后再订阅」会丢掉首条命令的块（T-0019 §11 风险 1）。
  // 同一 blockId 会来两次事件（running → 最终），一律按 id 覆盖。
  useEffect(
    () =>
      bridge.subscribeBlocks((block) => {
        setBlocks((previous) => upsertBlock(previous, block));
      }),
    [bridge],
  );

  // 首屏从 ?theme= 进入时也要落到 <html data-theme>；切换时同步（不刷新）。
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  // 每次改分都落 localStorage（key: np.m0.compare），刷新后保留。
  useEffect(() => {
    if (persist) {
      saveScores(scores);
    }
  }, [scores, persist]);

  useEffect(
    () => () => {
      if (feedbackTimer.current !== null) {
        window.clearTimeout(feedbackTimer.current);
      }
    },
    [],
  );

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
    // `clear` 在 `submitInput` 里截住：只清本地块，不发 IPC（命令表 §2.1）。
    void submitInput(input, {
      exec: (text, blockId) => bridge.exec(text, blockId),
      createBlockId,
      applyBlock: (block) => {
        setBlocks((previous) => upsertBlock(previous, block));
      },
      clear: () => {
        setBlocks([]);
      },
    });
  };

  const clear = (): void => {
    setBlocks([]);
  };

  const changeScore = (side: ScoreSide, dimension: DimensionId, value: number): void => {
    setScores((previous) => ({
      blocks:
        side === "blocks"
          ? { ...previous.blocks, [dimension]: value }
          : previous.blocks,
      xterm:
        side === "xterm"
          ? { ...previous.xterm, [dimension]: value }
          : previous.xterm,
    }));
  };

  const showFeedback = (message: string): void => {
    setFeedback(message);

    if (feedbackTimer.current !== null) {
      window.clearTimeout(feedbackTimer.current);
    }

    feedbackTimer.current = window.setTimeout(() => {
      feedbackTimer.current = null;
      setFeedback(null);
    }, 2000);
  };

  const exportScores = (): void => {
    const payload = buildExportPayload({
      scores,
      variant,
      theme,
      now: new Date(),
      resources: collectResourceEntries(),
    });

    void copyToClipboard(JSON.stringify(payload, null, 2)).then((copied) => {
      showFeedback(copied ? "评分 JSON 已复制" : "复制失败，请重试");
    });
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
          {LAB_THEMES.map((value) => (
            <button
              key={value}
              type="button"
              className="np-lab__chip"
              data-theme={value}
              aria-pressed={theme === value}
              onClick={() => selectTheme(value)}
            >
              {value}
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

      <ScorePanel
        scores={scores}
        onChange={changeScore}
        onExport={exportScores}
        collapsed={collapsed}
        onToggle={() => setCollapsed((previous) => !previous)}
        feedback={feedback}
      />
    </div>
  );
}
