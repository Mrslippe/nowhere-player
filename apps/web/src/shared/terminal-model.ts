/**
 * 终端输出模型的 TS 镜像（对应 ARCH-005）。
 *
 * 权威定义在 `crates/np-core/src/terminal.rs`；任何字段改动须同步两侧。
 * 字段名与 serde 序列化结果完全一致（camelCase），可直接 JSON.parse 后使用。
 */

/** 模型版本（ARCH-005 §5）。 */
export const SCHEMA_VERSION = 1;

/** 块状态，驱动输入行指示器。 */
export type BlockStatus = "pending" | "running" | "done" | "error" | "cancelled";

/** 整段文本的语义色；具体色值由渲染器决定（ARCH-005 §4）。 */
export type Tone = "accent" | "danger" | "warn" | "success" | "muted";

/** 行内片段的语义色；与 Tone 共用同一套语义。 */
export type SpanColor = Tone;

/** 行内片段：文本 + 语义样式，不含 HTML / ANSI。 */
export interface Span {
  text: string;
  bold: boolean;
  dim: boolean;
  italic: boolean;
  /** `null` 表示默认前景色。 */
  color: SpanColor | null;
}

/** 普通文本、键值行。 */
export interface TextOutput {
  type: "text";
  spans: Span[];
  /** 整段默认语义色；`null` 为默认前景色。 */
  tone: Tone | null;
}

/** 搜索结果、队列、库列表等表格。 */
export interface TableOutput {
  type: "table";
  columns: string[];
  /** 每行长度应与 `columns` 一致。 */
  rows: string[][];
  selectable: boolean;
}

/** 扫描、下载等可原地刷新的进度。 */
export interface ProgressOutput {
  type: "progress";
  label: string;
  current: number;
  /** `0` 表示总量未知。 */
  total: number;
  unit: string;
}

/** 失败输出。 */
export interface ErrorOutput {
  type: "error";
  code: string;
  message: string;
  hint: string | null;
  /** 可点击的建议动作（按钮文本）。 */
  actions: string[] | null;
}

/** 补全建议 / 纠错提示。 */
export interface HintOutput {
  type: "hint";
  text: string;
  suggestion: string | null;
}

/** `media.kind`：封面或缩略图。 */
export type MediaKind = "cover" | "thumb";

/** 封面 / 缩略图；xterm 渲染器降级为文字占位。 */
export interface MediaOutput {
  type: "media";
  kind: MediaKind;
  /** 资源引用；具体协议由上层约定。 */
  ref: string;
  /** 期望边长（像素）。 */
  size: number;
}

/** 六种输出原语。 */
export type Output =
  | TextOutput
  | TableOutput
  | ProgressOutput
  | ErrorOutput
  | HintOutput
  | MediaOutput;

/** 一条命令 = 一个块。进度通过 `id` 原地替换同一块。 */
export interface Block {
  /** 稳定标识，用于增量更新。 */
  id: string;
  /** 用户原始输入（含参数）。 */
  input: string;
  /** 开始时间，Unix 毫秒时间戳。 */
  startedAt: number;
  /** 结束耗时（毫秒）；运行中为 `null`。 */
  durationMs: number | null;
  status: BlockStatus;
  outputs: Output[];
  /** 尾注，如“3 条结果 · 输入 add 1-2 加入队列”。 */
  footer: string | null;
  /** 载荷版本；当前为 {@link SCHEMA_VERSION}。 */
  schemaVersion: number;
}
