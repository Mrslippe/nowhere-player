/**
 * T-0008 §6.2：假命令生成器。
 *
 * `input → Block`，不接 IPC、不解析真实命令、不访问网络。
 * 只认演示命令（`scan` / `search` / `play` / `perm` / `hint` / `cover` /
 * `help` / `version`）；其余输入一律返回 `E_CMD_UNKNOWN`，不做“输入什么都能跑”的假象。
 *
 * T-0019 起它只作为浏览器（无 Tauri）预览的兜底，由 `command-bridge` 的 `fallback`
 * 注入；终端窗默认走真实 `cmd.exec`。
 *
 * 一个命令 = 一个 Block；`progress` 等原语只是输出，不在这里做原地刷新。
 */
import type { Block, BlockStatus, Output, Span } from "../../shared/terminal-model";
import { SCHEMA_VERSION } from "../../shared/terminal-model";
import { APP_VERSION } from "./terminal-session";

interface DemoTrack {
  title: string;
  artist: string;
  duration: string;
}

const TRACKS: readonly DemoTrack[] = [
  { title: "晴天", artist: "周杰伦", duration: "4:29" },
  { title: "稻香", artist: "周杰伦", duration: "3:43" },
  { title: "夜航", artist: "林一", duration: "3:42" },
  { title: "富士山下", artist: "陈奕迅", duration: "4:19" },
  { title: "平凡之路", artist: "朴树", duration: "5:01" },
];

/** 演示时间戳：让种子块不随刷新变化（同 fixture 的 startedAt）。 */
export const DEMO_TIME = 1_762_512_000_000;

/** 新块 id 的递增序号，保证同一毫秒内多次提交也不撞 id。 */
let sequence = 0;

export interface MockCommandOptions {
  /** 覆盖开始时间，测试用。 */
  now?: number;
  /** 覆盖块 id，测试用。 */
  id?: string;
}

type MockKind =
  | "scan"
  | "search"
  | "play"
  | "perm"
  | "hint"
  | "cover"
  | "help"
  | "version"
  | "unknown";

interface ParsedCommand {
  kind: MockKind;
  query?: string;
  index?: number;
}

interface MockResult {
  outputs: Output[];
  status: BlockStatus;
  durationMs: number;
  footer: string | null;
}

function span(text: string, style: Partial<Omit<Span, "text">> = {}): Span {
  return {
    text,
    bold: false,
    dim: false,
    italic: false,
    color: null,
    ...style,
  };
}

function table(columns: string[], rows: string[][]): Output {
  return { type: "table", columns, rows, selectable: true };
}

function textOutput(spans: Span[]): Output {
  return { type: "text", tone: null, spans };
}

function trackRows(withPlayable: boolean): string[][] {
  return TRACKS.map((track, index) => {
    const base = [
      String(index + 1),
      track.title,
      track.artist,
      track.duration,
    ];
    return withPlayable ? [...base, "✓"] : base;
  });
}

function scanResult(): MockResult {
  return {
    status: "done",
    durationMs: 1842,
    outputs: [
      { type: "progress", label: "扫描 ~/Music", current: 1284, total: 1284, unit: "首" },
      table(["#", "曲名", "艺人", "时长"], trackRows(false)),
    ],
    footer: "1,284 首 · 输入 search 晴天 过滤",
  };
}

function searchResult(query: string): MockResult {
  const needle = query.toLowerCase();
  const matches = TRACKS.map((track, index) => ({ ...track, number: index + 1 })).filter(
    (track) => `${track.title}${track.artist}`.toLowerCase().includes(needle),
  );

  return {
    status: "done",
    durationMs: 9,
    outputs: [
      table(
        ["#", "曲名", "艺人", "时长", "可播"],
        matches.map((track) => [
          String(track.number),
          track.title,
          track.artist,
          track.duration,
          "✓",
        ]),
      ),
    ],
    footer:
      matches.length > 0
        ? `${matches.length} 条结果 · 输入 play ${matches[0].number} 播放第一条`
        : "0 条结果 · 换个关键词试试",
  };
}

function playResult(index: number): MockResult {
  const track = TRACKS[index - 1];

  if (track === undefined) {
    return {
      status: "error",
      durationMs: 3,
      outputs: [
        {
          type: "error",
          code: "E_CMD_ARG",
          message: `play 的序号超出范围：队列只有 ${TRACKS.length} 首`,
          hint: `用法：play 1-${TRACKS.length}`,
          actions: null,
        },
      ],
      footer: null,
    };
  }

  return {
    status: "done",
    durationMs: 4,
    outputs: [
      {
        type: "text",
        tone: null,
        spans: [
          span("正在播放 ", { dim: true }),
          span(track.title, { bold: true, color: "accent" }),
          span(` — ${track.artist} · ${track.duration}`, { dim: true, color: "muted" }),
        ],
      },
    ],
    footer: "mock 输出 · 不会真的播放",
  };
}

function permResult(): MockResult {
  return {
    status: "error",
    durationMs: 6,
    outputs: [
      {
        type: "error",
        code: "E_LIB_PERMISSION",
        message: "无法读取目录 ~/Music/Private",
        hint: "检查文件权限，或输入 scan --skip-private",
        actions: ["重试", "跳过该目录"],
      },
    ],
    footer: null,
  };
}

function hintResult(): MockResult {
  return {
    status: "done",
    durationMs: 2,
    outputs: [
      {
        type: "hint",
        text: '路径含空格时用引号：scan "~/My Music"',
        suggestion: "按 Tab 采纳",
      },
    ],
    footer: null,
  };
}

function coverResult(): MockResult {
  return {
    status: "done",
    durationMs: 5,
    outputs: [
      {
        type: "media",
        kind: "cover",
        ref: "np://library/album/7f3c/cover.jpg",
        size: 48,
      },
    ],
    footer: "media 原语：A 侧渐变占位，B 侧降级为 [cover]",
  };
}

/** 浏览器预览也没有可跑的命令表：`help` 至少要给出一份能照抄的清单。 */
const MOCK_HELP: readonly (readonly [string, string])[] = [
  ["play [序号|路径]", "播放或继续当前"],
  ["pause / resume", "暂停 / 继续"],
  ["next / prev", "队列上一首 / 下一首"],
  ["seek <ms|mm:ss|±s>", "跳转"],
  ["vol <0-100|±N>", "音量"],
  ["queue [list|add|remove|clear|jump]", "队列"],
  ["mode repeat <off|one|all>", "循环模式"],
  ["mode shuffle <on|off>", "随机模式"],
  ["help [命令]", "命令表"],
  ["version", "版本"],
  ["clear", "清屏（前端本地处理）"],
];

function helpResult(): MockResult {
  return {
    status: "done",
    durationMs: 2,
    outputs: MOCK_HELP.map(([usage, summary]) =>
      textOutput([span(usage, { bold: true }), span(`  ${summary}`)]),
    ),
    footer: "浏览器 mock · 真实后端（T-0018）返回完整命令表",
  };
}

function versionResult(): MockResult {
  return {
    status: "done",
    durationMs: 1,
    outputs: [
      textOutput([
        span("Nowhere Player", { bold: true, color: "accent" }),
        span(` ${APP_VERSION}`),
      ]),
    ],
    footer: null,
  };
}

function unknownResult(input: string): MockResult {
  const trimmed = input.trim();

  return {
    status: "error",
    durationMs: 1,
    outputs: [
      {
        type: "error",
        code: "E_CMD_UNKNOWN",
        message: trimmed === "" ? "空输入不是命令" : `未识别的命令：${trimmed}`,
        hint: "did you mean 'scan'?",
        actions: null,
      },
    ],
    footer: null,
  };
}

/** 只做最小解析：命令名 + 参数，不实现真正的参数语法。 */
function parseCommand(input: string): ParsedCommand {
  const normalized = input.trim().replace(/\s+/g, " ");
  const [head = "", ...restParts] = normalized.split(" ");
  const rest = restParts.join(" ");

  switch (head.toLowerCase()) {
    case "scan":
      return rest === "" ? { kind: "scan" } : { kind: "unknown" };
    case "search":
      return rest === "" ? { kind: "unknown" } : { kind: "search", query: rest };
    case "play": {
      const match = /^([0-9]+)$/.exec(rest);
      return match === null
        ? { kind: "unknown" }
        : { kind: "play", index: Number(match[1]) };
    }
    case "perm":
      return rest === "" ? { kind: "perm" } : { kind: "unknown" };
    case "hint":
      return rest === "" ? { kind: "hint" } : { kind: "unknown" };
    case "cover":
      return rest === "" ? { kind: "cover" } : { kind: "unknown" };
    case "help":
      // mock 不解析子命令：带参数也给全表（真实后端会只给该命令的用法）。
      return { kind: "help" };
    case "version":
      return rest === "" ? { kind: "version" } : { kind: "unknown" };
    default:
      return { kind: "unknown" };
  }
}

function resolve(input: string): MockResult {
  const parsed = parseCommand(input);

  switch (parsed.kind) {
    case "scan":
      return scanResult();
    case "search":
      return searchResult(parsed.query ?? "");
    case "play":
      return playResult(parsed.index ?? 0);
    case "perm":
      return permResult();
    case "hint":
      return hintResult();
    case "cover":
      return coverResult();
    case "help":
      return helpResult();
    case "version":
      return versionResult();
    case "unknown":
      return unknownResult(input);
  }
}

function createId(now: number): string {
  sequence += 1;
  return `blk_mock_${now.toString(36)}_${sequence.toString(36)}`;
}

/**
 * 把一条演示命令包装成契约里的 {@link Block}。
 *
 * 八个命令：`scan` / `search <关键词>` / `play <序号>` / `perm` / `hint` /
 * `cover` / `help` / `version`。
 * 其他输入返回 `E_CMD_UNKNOWN`。
 */
export function runMockCommand(input: string, options: MockCommandOptions = {}): Block {
  const now = options.now ?? Date.now();
  const result = resolve(input);

  return {
    schemaVersion: SCHEMA_VERSION,
    id: options.id ?? createId(now),
    input: input.trim(),
    startedAt: now,
    durationMs: result.durationMs,
    status: result.status,
    outputs: result.outputs,
    footer: result.footer,
  };
}
