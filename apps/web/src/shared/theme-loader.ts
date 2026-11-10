/**
 * 主题加载器与安全模式（T-0029 §6.2–§6.4）。
 *
 * 职责链：`theme.apply` IPC → 把 tokens 写成 `<html>` 的 inline CSS 变量 →
 * 300ms 后检查哨兵元素尺寸 → 崩坏则回退 graphite 并推一条 `E_THEME_BROKEN` 错误块。
 *
 * 三条硬约束：
 * 1. **只写 inline style**，绝不注入 `<style>` 字符串（CSP 与可回滚性）。
 * 2. 每次应用都先清掉上一份主题写入的变量，避免残留变量串味。
 * 3. 安全模式**只尝试一次**：回退到 graphite 后不再重试，也不保留会再次崩坏的选择。
 *
 * 副作用（DOM / 定时器 / 存储 / IPC / 事件）全部经 {@link ThemeEnvironment} 注入，
 * 单测用最小 stub，不依赖 jsdom（任务卡 §8）。
 */
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import {
  isTauriRuntime,
  TERMINAL_BLOCK_EVENT,
  toCommandError,
  type CommandError,
} from "./command-bridge";
import { SCHEMA_VERSION, type Block } from "./terminal-model";

/** IPC 命令名（与 `apps/desktop/src-tauri/src/ipc.rs` 的 rename 一致）。 */
export const THEME_LIST_COMMAND = "theme.list";
/** 见 {@link THEME_LIST_COMMAND}。 */
export const THEME_APPLY_COMMAND = "theme.apply";
/** 见 {@link THEME_LIST_COMMAND}。 */
export const THEME_EXPORT_COMMAND = "theme.exportBuiltin";
/** 从 settings 读回启动主题（T-0042）。 */
export const THEME_INITIAL_COMMAND = "theme.initial";
/** 设置命令切换主题后广播；前端监听并应用。 */
export const THEME_CHANGED_EVENT = "theme:changed";

/** 选择持久化的 localStorage 键（与 `shared/theme.ts` 的 `np.theme` 区分开）。 */
export const THEME_STORAGE_KEY = "np.theme.id";

/** 安全模式：应用后等待多久再做哨兵检查（任务卡 §6.3）。 */
export const SENTINEL_DELAY_MS = 300;

/** 对比度告警阈值（WCAG AA 正文）。 */
export const MIN_CONTRAST_RATIO = 4.5;

/** 哨兵选择器：优先两个窗口都存在的根节点，退而求其次用播放器卡片。 */
export const SENTINEL_SELECTOR = "[data-window]";
/** 哨兵兜底选择器（任务卡 §6.3 原文；终端窗没有卡片，所以只作为兜底）。 */
export const FALLBACK_SENTINEL_SELECTOR = ".np-player-card";

/** 结构皮肤枚举（ADR-0003：v1 只有 plain / phosphor）。 */
export const THEME_SKINS = ["plain", "phosphor"] as const;

/** 结构皮肤。 */
export type Skin = (typeof THEME_SKINS)[number];

/** 主题来源。 */
export type ThemeSource = "builtin" | "user";

/** 内置主题 id（与 `data-theme` 取值一致；内置主题由 themes.css 兜底）。 */
export const BUILTIN_THEME_IDS = ["graphite", "amber", "ice", "phosphor"] as const;

/** 颜色 token 名（camelCase，与 np-core 的 `THEME_TOKEN_NAMES` 一致）。 */
export const THEME_TOKEN_NAMES = [
  "bg",
  "bgElev",
  "bgHover",
  "fg",
  "fgMuted",
  "fgFaint",
  "accent",
  "accentSoft",
  "accentFg",
  "danger",
  "warn",
  "success",
  "line",
  "lineStrong",
] as const;

/** 单个颜色 token 名。 */
export type ThemeTokenName = (typeof THEME_TOKEN_NAMES)[number];

/** 14 个颜色 token 的完整集合。 */
export type ThemeTokens = Record<ThemeTokenName, string>;

/** `theme.list` 的一项。 */
export interface ThemeListEntry {
  id: string;
  name: string;
  skin: Skin;
  source: ThemeSource;
}

/** `theme.apply` 的回执。 */
export interface ThemeApplyReply {
  id: string;
  name: string;
  skin: Skin;
  source: ThemeSource;
  tokens: ThemeTokens;
  warnings: string[];
}

/** `theme.exportBuiltin` 的回执。 */
export interface ThemeExportReply {
  path: string;
  created: boolean;
}

/** 可注入的 DOM 根（`document.documentElement` 的最小子集）。 */
export interface ThemeRoot {
  style: {
    setProperty(name: string, value: string): void;
    removeProperty(name: string): void;
  };
  dataset: Record<string, string | undefined>;
}

/** 哨兵元素的最小子集。 */
export interface ThemeSentinel {
  getBoundingClientRect(): { width: number; height: number };
}

/** 可注入的 localStorage（单测用内存实现）。 */
export interface ThemeStorage {
  get(key: string): string | null;
  set(key: string, value: string): void;
  remove(key: string): void;
}

/** 主题加载器依赖的环境；生产实现见 {@link browserEnvironment}。 */
export interface ThemeEnvironment {
  root: ThemeRoot;
  querySelector(selector: string): ThemeSentinel | null;
  /** 窗口是否隐藏（隐藏窗口量不到尺寸，检查应跳过而不是判崩坏）。 */
  isHidden(): boolean;
  /** 延迟执行（单测手动触发，断言 300ms 这个约定值）。 */
  schedule(callback: () => void, delayMs: number): void;
  /** 往终端推一条块（主题提示复用 `terminal:block` 通道）。 */
  emitBlock(block: Block): void;
  storage: ThemeStorage;
  /** `theme.apply` 的 IPC 通道（单测注入假实现）。 */
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  /** 监听 `theme:changed`；单测注入后手动触发 handler。 */
  onThemeChanged?(handler: (id: string) => void): Promise<() => void> | (() => void);
}

/** 应用主题的选项。 */
export interface ApplyThemeOptions {
  env?: ThemeEnvironment;
  /** 注入后不再走真 Tauri（单测用）。 */
  transport?: ThemeTransport;
}

/** 主题 IPC 传输层（只覆盖本模块用到的三个命令）。 */
export interface ThemeTransport {
  list(): Promise<ThemeListEntry[]>;
  apply(id: string): Promise<ThemeApplyReply>;
  exportBuiltin(id: string): Promise<ThemeExportReply>;
  /** 启动主题（settings.json 的 theme.id）；单测可不实现。 */
  initial?(): Promise<string | null>;
}

/** camelCase token → CSS 自定义属性名（`bgElev` → `--bg-elev`）。 */
export function cssVarName(token: ThemeTokenName): string {
  return `--${token.replace(/[A-Z]/g, (character) => `-${character.toLowerCase()}`)}`;
}

/** 是否是内置主题 id。 */
export function isBuiltinThemeId(id: string): boolean {
  return (BUILTIN_THEME_IDS as readonly string[]).includes(id);
}

/** 颜色格式校验：与 np-core 的 `is_valid_theme_color` 同规则（前端再挡一层）。 */
export function isThemeColor(value: string): boolean {
  const trimmed = value.trim();

  if (/^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/.test(trimmed)) {
    return true;
  }

  const rgba = /^rgba\(([^()]*)\)$/i.exec(trimmed);
  if (rgba === null) {
    return false;
  }

  const parts = rgba[1].split(",").map((part) => part.trim());
  if (parts.length !== 4) {
    return false;
  }

  const channelsOk = parts
    .slice(0, 3)
    .every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255);
  const alpha = parts[3] === "" ? Number.NaN : Number(parts[3]);

  return channelsOk && Number.isFinite(alpha) && alpha >= 0 && alpha <= 1;
}

/** 清掉上一份主题写进 `<html>` 的 inline 变量（只碰我们自己会写的名字）。 */
export function clearInlineTheme(env: ThemeEnvironment): void {
  for (const token of THEME_TOKEN_NAMES) {
    env.root.style.removeProperty(cssVarName(token));
  }
  env.root.style.removeProperty("color-scheme");
}

/**
 * 把一份主题写成 inline CSS 变量 + `data-skin` / `data-theme`。
 *
 * 返回实际写入的属性名（单测用来断言“非法值被跳过”）。`tokens` 里缺失或不合法
 * 的项会被跳过，此时该变量回到 CSS 里的默认值——不认识的 token 永远写不进去。
 */
export function applyTheme(theme: ThemeApplyReply, env: ThemeEnvironment): string[] {
  clearInlineTheme(env);

  const applied: string[] = [];
  for (const token of THEME_TOKEN_NAMES) {
    const value = theme.tokens?.[token];
    if (typeof value !== "string" || !isThemeColor(value)) {
      continue;
    }
    const name = cssVarName(token);
    env.root.style.setProperty(name, value.trim());
    applied.push(name);
  }

  env.root.dataset.skin = theme.skin === "phosphor" ? "phosphor" : "plain";
  env.root.dataset.theme = theme.id;
  // 四套主题都是暗色（DES-001）；同步 color-scheme，滚动条/表单控件跟随。
  env.root.style.setProperty("color-scheme", "dark");

  return applied;
}

/**
 * 回退到内置 graphite：清掉 inline 变量，交回 CSS 的 `[data-theme="graphite"]`。
 *
 * 这是安全模式的**唯一**回退目标：内置主题永远可用，不依赖用户文件。
 */
export function resetToGraphite(env: ThemeEnvironment): void {
  clearInlineTheme(env);
  env.root.dataset.theme = "graphite";
  env.root.dataset.skin = "plain";
  env.root.style.setProperty("color-scheme", "dark");
}

/** 解析不透明色（`#RGB` / `#RRGGBB`）；带 alpha 的形式返回 `null`（不参与对比度）。 */
export function parseOpaqueColor(
  value: string,
): { r: number; g: number; b: number } | null {
  const trimmed = value.trim();
  const short = /^#([0-9a-fA-F]{3})$/.exec(trimmed);
  if (short !== null) {
    const [r, g, b] = short[1].split("").map((digit) => parseInt(digit + digit, 16));
    return { r, g, b };
  }

  const long = /^#([0-9a-fA-F]{6})$/.exec(trimmed);
  if (long !== null) {
    const hex = long[1];
    return {
      r: parseInt(hex.slice(0, 2), 16),
      g: parseInt(hex.slice(2, 4), 16),
      b: parseInt(hex.slice(4, 6), 16),
    };
  }

  return null;
}

function relativeLuminance(color: { r: number; g: number; b: number }): number {
  const channel = (value: number): number => {
    const normalized = value / 255;
    return normalized <= 0.03928
      ? normalized / 12.92
      : ((normalized + 0.055) / 1.055) ** 2.4;
  };

  return (
    0.2126 * channel(color.r) +
    0.7152 * channel(color.g) +
    0.0722 * channel(color.b)
  );
}

/**
 * WCAG 对比度（`fg` / `bg`）。
 *
 * 任一方是 `rgba(...)` / `#RRGGBBAA` 时返回 `null`：需要与底色做 alpha 合成，
 * 这里不做近似，宁可不出提示也不误报。
 */
export function contrastRatio(foreground: string, background: string): number | null {
  const fg = parseOpaqueColor(foreground);
  const bg = parseOpaqueColor(background);

  if (fg === null || bg === null) {
    return null;
  }

  const lighter = Math.max(relativeLuminance(fg), relativeLuminance(bg));
  const darker = Math.min(relativeLuminance(fg), relativeLuminance(bg));

  return (lighter + 0.05) / (darker + 0.05);
}

/** 对比度不足时推的 `hint` 块（不阻止应用，任务卡 §2.4）。 */
export function contrastHintBlock(
  themeId: string,
  ratio: number,
  now: number = Date.now(),
): Block {
  return {
    schemaVersion: SCHEMA_VERSION,
    id: "blk_theme_contrast",
    input: "",
    startedAt: now,
    durationMs: null,
    status: "done",
    outputs: [
      {
        type: "hint",
        text: `主题 ${themeId} 的 fg/bg 对比度 ${ratio.toFixed(2)}:1，低于 ${MIN_CONTRAST_RATIO}:1`,
        suggestion: "调亮 tokens.fg 或压暗 tokens.bg",
      },
    ],
    footer: null,
  };
}

/** 安全模式回退时推的错误块（任务卡 §6.3）。 */
export function themeBrokenBlock(
  themeId: string,
  now: number = Date.now(),
): Block {
  return {
    schemaVersion: SCHEMA_VERSION,
    id: "blk_theme_broken",
    input: "",
    startedAt: now,
    durationMs: null,
    status: "error",
    outputs: [
      {
        type: "error",
        code: "E_THEME_BROKEN",
        message: `主题 ${themeId} 应用后布局崩坏，已回退 graphite`,
        hint: "检查该主题的 tokens 是否让哨兵元素宽高归零；修正文件后重新应用",
        actions: null,
      },
    ],
    footer: null,
  };
}

/** 应用主题失败（IPC / 文件 / 校验）时推的错误块。 */
export function themeErrorBlock(error: ThemeFailure, now: number = Date.now()): Block {
  return {
    schemaVersion: SCHEMA_VERSION,
    id: "blk_theme_error",
    input: "",
    startedAt: now,
    durationMs: null,
    status: "error",
    outputs: [
      {
        type: "error",
        code: error.code,
        message: `主题应用失败：${error.message}`,
        hint: error.hint ?? null,
        actions: null,
      },
    ],
    footer: null,
  };
}

/** 主题失败：IPC 错误加上 Rust 侧的可选 `hint`（`toCommandError` 不保留它）。 */
export interface ThemeFailure extends CommandError {
  hint?: string | null;
}

/** 把任意异常归一成 `{ code, message, hint? }`。 */
export function toThemeFailure(error: unknown): ThemeFailure {
  const base = toCommandError(error);
  const hint =
    typeof error === "object" &&
    error !== null &&
    "hint" in error &&
    typeof (error as { hint: unknown }).hint === "string"
      ? (error as { hint: string }).hint
      : null;

  return { ...base, hint };
}

/** 安全模式检查结果。 */
export type SafetyResult = "ok" | "broken" | "skipped";

/**
 * 哨兵检查（安全模式的核心，**只执行一次**）。
 *
 * - 窗口隐藏 / 找不到哨兵元素：`skipped`（不判崩坏，避免隐藏窗误回退）；
 * - 哨兵宽或高为 0：判崩坏，回退 graphite + 推 `E_THEME_BROKEN`；
 * - 否则 `ok`。
 */
export function checkSentinel(
  themeId: string,
  env: ThemeEnvironment,
  now: () => number = Date.now,
): SafetyResult {
  if (env.isHidden()) {
    return "skipped";
  }

  const sentinel =
    env.querySelector(SENTINEL_SELECTOR) ?? env.querySelector(FALLBACK_SENTINEL_SELECTOR);
  if (sentinel === null) {
    return "skipped";
  }

  const rect = sentinel.getBoundingClientRect();
  if (rect.width > 0 && rect.height > 0) {
    return "ok";
  }

  resetToGraphite(env);
  // 把会崩坏的选择清掉，下次启动不再自动踩坑（任务卡 §6.3“只尝试一次”）。
  env.storage.remove(THEME_STORAGE_KEY);
  env.emitBlock(themeBrokenBlock(themeId, now()));
  console.warn(`[theme] ${themeId} 应用后布局崩坏（哨兵尺寸为 0），已回退 graphite`);

  return "broken";
}

/** 生成主题 IPC 的真实传输层（Tauri `invoke`）。 */
export function tauriThemeTransport(): ThemeTransport {
  return {
    list: () => invoke<ThemeListEntry[]>(THEME_LIST_COMMAND),
    apply: (id) => invoke<ThemeApplyReply>(THEME_APPLY_COMMAND, { id }),
    exportBuiltin: (id) =>
      invoke<ThemeExportReply>(THEME_EXPORT_COMMAND, { id }),
    initial: () => invoke<string | null>(THEME_INITIAL_COMMAND),
  };
}

/** 选择读写的默认存储（localStorage 不可用时静默降级为空实现）。 */
function browserStorage(): ThemeStorage {
  const memory = new Map<string, string>();

  return {
    get(key) {
      try {
        const value = globalThis.localStorage?.getItem(key);
        if (typeof value === "string") {
          return value;
        }
      } catch {
        // 隐私模式 / 配额问题：退化到内存。
      }
      return memory.get(key) ?? null;
    },
    set(key, value) {
      memory.set(key, value);
      try {
        globalThis.localStorage?.setItem(key, value);
      } catch {
        // 写不进去也不影响本次会话。
      }
    },
    remove(key) {
      memory.delete(key);
      try {
        globalThis.localStorage?.removeItem(key);
      } catch {
        // 同上。
      }
    },
  };
}

/** 把块推给终端窗：复用 `terminal:block` 通道（本卡无权改 `features/terminal/**`）。 */
async function emitBlockToTerminal(block: Block): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }

  try {
    await emit(TERMINAL_BLOCK_EVENT, block);
  } catch {
    // 终端窗可能还没创建（懒建）；提示丢失不影响主题本身。
  }
}

/** 生产环境：真实 document / window / localStorage / Tauri。 */
export function browserEnvironment(): ThemeEnvironment | null {
  if (typeof document === "undefined") {
    return null;
  }

  const root = document.documentElement;

  return {
    root: { style: root.style, dataset: root.dataset },
    querySelector: (selector) => document.querySelector<HTMLElement>(selector),
    isHidden: () =>
      typeof document.visibilityState === "string" &&
      document.visibilityState === "hidden",
    schedule: (callback, delayMs) => {
      window.setTimeout(callback, delayMs);
    },
    emitBlock: (block) => {
      void emitBlockToTerminal(block);
    },
    storage: browserStorage(),
    invoke: (command, args) => invoke(command, args),
    onThemeChanged: (handler) =>
      listen<{ id?: unknown }>(THEME_CHANGED_EVENT, (event) => {
        const id =
          typeof event.payload?.id === "string" ? event.payload.id.trim() : "";
        if (id !== "") {
          handler(id);
        }
      }),
  };
}

/** 校验 `theme.apply` 回执：形状不对就当作失败，绝不写脏值进 CSS。 */
export function isThemeApplyReply(value: unknown): value is ThemeApplyReply {
  if (typeof value !== "object" || value === null) {
    return false;
  }

  const reply = value as Record<string, unknown>;
  if (typeof reply.id !== "string" || typeof reply.name !== "string") {
    return false;
  }
  if (reply.skin !== "plain" && reply.skin !== "phosphor") {
    return false;
  }
  if (reply.source !== "builtin" && reply.source !== "user") {
    return false;
  }
  if (typeof reply.tokens !== "object" || reply.tokens === null) {
    return false;
  }

  return THEME_TOKEN_NAMES.every(
    (token) => typeof (reply.tokens as Record<string, unknown>)[token] === "string",
  );
}

/** 选择持久化的读取（只认非内置 id）。 */
export function loadSelectedThemeId(
  storage: ThemeStorage = browserStorage(),
): string | null {
  const stored = storage.get(THEME_STORAGE_KEY)?.trim() ?? "";
  return stored === "" || isBuiltinThemeId(stored) ? null : stored;
}

/** 选择持久化的写入。 */
export function saveSelectedThemeId(
  id: string,
  storage: ThemeStorage = browserStorage(),
): void {
  if (id !== "" && !isBuiltinThemeId(id)) {
    storage.set(THEME_STORAGE_KEY, id);
  }
}

/** 清掉选择（回退 / 用户改回内置主题时）。 */
export function clearSelectedThemeId(
  storage: ThemeStorage = browserStorage(),
): void {
  storage.remove(THEME_STORAGE_KEY);
}

/** 当前代次：后一次应用会让前一次的哨兵检查失效，避免误回退新主题。 */
let generation = 0;

/** 启动安装去重（StrictMode 会双跑 effect）。 */
let installed = false;

/**
 * 应用主题 id 并把回执落到 DOM。
 *
 * 失败（IPC 失败、文件缺失/非法）时推一条错误块并返回 `null`，不抛出。
 */
export async function applyThemeById(
  id: string,
  options: ApplyThemeOptions = {},
): Promise<ThemeApplyReply | null> {
  const env = options.env ?? browserEnvironment();
  const trimmed = id.trim();
  if (env === null || trimmed === "") {
    return null;
  }

  const transport = options.transport;

  let reply: unknown;
  try {
    reply = transport
      ? await transport.apply(trimmed)
      : await env.invoke<unknown>(THEME_APPLY_COMMAND, { id: trimmed });
  } catch (error) {
    env.emitBlock(themeErrorBlock(toThemeFailure(error)));
    return null;
  }

  if (!isThemeApplyReply(reply)) {
    env.emitBlock(
      themeErrorBlock({
        code: "E_THEME_JSON",
        message: "theme.apply 返回了非法回执",
      }),
    );
    return null;
  }

  const theme = reply;
  applyTheme(theme, env);

  if (theme.source === "user") {
    saveSelectedThemeId(theme.id, env.storage);
  } else {
    clearSelectedThemeId(env.storage);
  }

  for (const warning of theme.warnings) {
    console.warn(`[theme] ${theme.id}: ${warning}`);
  }

  const ratio = contrastRatio(theme.tokens.fg, theme.tokens.bg);
  if (ratio !== null && ratio < MIN_CONTRAST_RATIO) {
    env.emitBlock(contrastHintBlock(theme.id, ratio));
  }

  // 安全模式：300ms 后检查一次；只有仍是当前代次才允许回退。
  const current = ++generation;
  env.schedule(() => {
    if (current !== generation) {
      return;
    }
    checkSentinel(theme.id, env);
  }, SENTINEL_DELAY_MS);

  return theme;
}

/** `theme.list`：内置 + 用户主题。 */
export async function listThemes(
  options: ApplyThemeOptions = {},
): Promise<ThemeListEntry[]> {
  const transport = options.transport;
  if (transport !== undefined) {
    return transport.list();
  }
  if (!isTauriRuntime()) {
    return [];
  }
  try {
    return await invoke<ThemeListEntry[]>(THEME_LIST_COMMAND);
  } catch {
    return [];
  }
}

/** `theme.exportBuiltin`：把内置主题导出成用户可改的 JSON。 */
export async function exportBuiltinTheme(
  id: string,
  options: ApplyThemeOptions = {},
): Promise<ThemeExportReply | null> {
  const transport = options.transport;
  if (transport !== undefined) {
    return transport.exportBuiltin(id.trim());
  }
  if (!isTauriRuntime()) {
    return null;
  }
  try {
    return await invoke<ThemeExportReply>(THEME_EXPORT_COMMAND, { id: id.trim() });
  } catch {
    return null;
  }
}

/** 启动安装选项。 */
export interface InstallThemeLoaderOptions extends ApplyThemeOptions {
  /** 测试用：跳过 Tauri 运行时判断（必须同时注入 transport）。 */
  force?: boolean;
  /** 启动时先读 settings.json 的 theme.id（T-0042）；URL dev 预览仍由 App 跳过 initial。 */
  preferSettings?: boolean;
  /** 只装 `theme:changed` 监听，不应用启动主题（dev 预览时用）。 */
  skipInitial?: boolean;
}

/**
 * 启动时恢复上次选择（任务卡 §5.1 的“入口/路由”注册点）。
 *
 * 优先级：
 * 1. URL `?theme=<用户主题 id>`：显式深链，优先；
 * 2. URL `?theme=<内置 id>` 或 `?skin=`：交给 CSS / dev 预览，不写 inline；
 * 3. localStorage `np.theme.id`：上次选中的用户主题。
 *
 * 只有**用户主题**会被恢复：内置主题的持久化仍由 `shared/theme.ts` + `?theme=`
 * 负责，避免两套机制互相覆盖（任务卡 §6.2）。
 */
export async function installThemeLoader(
  options: InstallThemeLoaderOptions = {},
): Promise<ThemeApplyReply | null> {
  if (installed) {
    return null;
  }

  const env = options.env ?? browserEnvironment();
  if (env === null) {
    return null;
  }
  if (!options.force && options.transport === undefined && !isTauriRuntime()) {
    return null;
  }

  installed = true;

  // 先装监听：即使 dev 预览跳过启动主题，`theme set` 也要能立即刷新本窗口。
  if (env.onThemeChanged) {
    try {
      const unlisten = env.onThemeChanged((id) => {
        void applyThemeById(id, { env, transport: options.transport });
      });
      if (unlisten instanceof Promise) {
        void unlisten.catch(() => {});
      }
    } catch {
      // 监听失败不影响启动主题；下一次命令仍会写 settings。
    }
  }

  if (options.skipInitial) {
    return null;
  }

  if (options.preferSettings) {
    const saved = await readInitialThemeId(env, options.transport);
    if (saved !== null) {
      return applyThemeById(saved, { env, transport: options.transport });
    }
  }

  const urlTheme = readUrlTheme();
  const id =
    urlTheme === null
      ? loadSelectedThemeId(env.storage)
      : isBuiltinThemeId(urlTheme)
        ? null
        : urlTheme;

  if (id === null) {
    return null;
  }

  return applyThemeById(id, { env, transport: options.transport });
}

async function readInitialThemeId(
  env: ThemeEnvironment,
  transport: ThemeTransport | undefined,
): Promise<string | null> {
  try {
    const value =
      transport?.initial !== undefined
        ? await transport.initial()
        : await env.invoke<string | null>(THEME_INITIAL_COMMAND);
    return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
  } catch {
    return null;
  }
}

function readUrlTheme(): string | null {
  if (typeof window === "undefined") {
    return null;
  }

  try {
    const value = new URLSearchParams(window.location.search).get("theme");
    const trimmed = value?.trim() ?? "";
    return trimmed === "" ? null : trimmed;
  } catch {
    return null;
  }
}

/** 单测用：复位“已安装”与代次状态。 */
export function resetThemeLoader(): void {
  installed = false;
  generation = 0;
}
