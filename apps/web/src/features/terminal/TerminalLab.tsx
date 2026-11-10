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
import { WindowControls } from "./components/WindowControls";
import {
  DEFAULT_SCORES,
  loadScores,
  saveScores,
  type ComparisonScores,
  type DimensionId,
  type ScoreSide,
} from "./components/scoring";
import { TerminalStage } from "./components/Stage";
import { parseLab } from "./components/lab";
import { useLibraryStats } from "./components/use-library-stats";
import {
  applyTheme,
  LAB_THEMES,
  parseTheme,
  type LabTheme,
} from "./components/theme";
import {
  DEFAULT_VARIANT,
  LAB_VARIANTS,
  parseVariant,
  VARIANT_LABELS,
  type LabVariant,
} from "./components/variant";
import { DEMO_TIME, runMockCommand } from "./mock-commands";
import {
  createBlockId,
  createWelcomeBlock,
  loadAppVersion,
  submitInput,
  withStreamLost,
  withWelcomeVersion,
} from "./terminal-session";
import "./terminal.css";

function readParam(name: string): string | null {
  if (typeof window === "undefined") {
    return null;
  }

  return new URLSearchParams(window.location.search).get(name);
}

/** 完整 query，交给只读的 parseLab。 */
function readSearch(): string | null {
  return typeof window === "undefined" ? null : window.location.search;
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
  /** 测试/预览用：覆盖 ?lab=1；不传则按 URL（含兼容的 ?variant=）判断。 */
  initialLab?: boolean;
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
  initialLab,
  initialBlocks,
  initialScores,
  persist = true,
}: TerminalLabProps) {
  /*
   * 产品视图默认只留块流；M0 对比实验台（视图/主题/评分/pane 头）
   * 收进 ?lab=1。显式传 initialVariant 的测试/预览也算实验台；
   * 旧 ?variant= 直链继续可用（parseLab 兼容）。
   */
  const lab = initialLab ?? (initialVariant !== undefined || parseLab(readSearch()));
  const library = useLibraryStats();

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
    () =>
      initialVariant ??
      (lab ? parseVariant(readParam("variant")) : DEFAULT_VARIANT),
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
  /**
   * 最近一次 `clear` 的时间戳（毫秒；未清空为 null）。
   *
   * `clear` 只清视图、不发 IPC，所以清空前的命令可能还有块在路上；
   * 晚到的块按 `startedAt` 判断是否属于清空前，属于则丢弃（T-0021 §2.3）。
   */
  const clearedAtRef = useRef<number | null>(null);

  /** 应用一条块；`clear` 之前开始的在途块在这里被丢弃。 */
  const applyBlock = (block: Block): void => {
    setBlocks((previous) => upsertBlock(previous, block, clearedAtRef.current));
  };

  /** 清空本地块并记下时间戳；`clear` 命令与顶栏按钮共用同一入口。 */
  const clearBlocks = (): void => {
    clearedAtRef.current = Date.now();
    setBlocks([]);
  };

  // 订阅必须在挂载时就建立：`cmd.exec` 在返回回执前就 emit，
  // 「await exec 之后再订阅」会丢掉首条命令的块（T-0019 §11 风险 1）。
  // 同一 blockId 会来两次事件（running → 最终），一律按 id 覆盖。
  useEffect(
    () =>
      bridge.subscribeBlocks(applyBlock, () => {
        // 订阅失败：推一条本地 error 块，明确告诉用户实时输出不可用。
        // 不自动重试（避免刷屏）；重开终端窗 = 重新订阅，是唯一的重试路径。
        setBlocks((previous) => withStreamLost(previous));
      }),
    [bridge],
  );

  // 版本号异步读取：首屏先用兜底版本渲染，拿到真版本后替换（T-0021 §2.4）。
  useEffect(() => {
    let active = true;

    void loadAppVersion().then((version) => {
      if (active) {
        setBlocks((previous) => withWelcomeVersion(previous, version));
      }
    });

    return () => {
      active = false;
    };
  }, []);

  // 只有实验台才覆盖主题；产品视图沿用用户持久化 / 全局主题（T-0041）。
  useEffect(() => {
    if (lab) {
      applyTheme(theme);
    }
  }, [theme, lab]);

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
      applyBlock,
      clear: clearBlocks,
    });
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
    <div
      className="np-lab"
      data-testid="terminal-lab"
      data-variant={variant}
      data-lab={lab ? "true" : "false"}
    >
      <header className="np-lab__topbar" data-tauri-drag-region="deep">
        <span className="np-lab__brand">
          Nowhere Player
          <span className="np-lab__brand-suffix">终端</span>
        </span>

        <span
          className="np-lab__status"
          data-tone={library.state === "ready" ? "ready" : "muted"}
          title="本地库曲目数"
        >
          <i className="np-lab__status-dot" aria-hidden="true" />
          {library.state === "loading"
            ? "本地库…"
            : library.state === "ready"
              ? `本地 ${library.tracks ?? 0} 首`
              : "本地库未连接"}
        </span>

        {lab ? (
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
        ) : null}

        {lab ? (
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
        ) : null}

        <span className="np-lab__spacer" />

        <Button variant="ghost" size="sm" onClick={clearBlocks}>
          清空
        </Button>

        <WindowControls />
      </header>

      <TerminalStage
        variant={variant}
        blocks={blocks}
        lab={lab}
        onRunAction={submit}
      />

      <CommandInput
        onSubmit={submit}
        complete={(input, cursor) => bridge.complete(input, cursor)}
        persistHistory={persist}
      />

      {!lab ? (
        <div className="np-lab__statusbar" aria-label="终端状态">
          <span>↑↓ 历史　Tab 补全　Enter 执行</span>
          <span>
            {library.state === "ready"
              ? `本地 ${library.tracks ?? 0} 首`
              : "本地库未连接"}
          </span>
        </div>
      ) : null}

      {lab ? (
        <ScorePanel
          scores={scores}
          onChange={changeScore}
          onExport={exportScores}
          collapsed={collapsed}
          onToggle={() => setCollapsed((previous) => !previous)}
          feedback={feedback}
        />
      ) : null}
    </div>
  );
}
