import type { Block, BlockStatus, Output } from "../../../../shared/terminal-model";
import { normalizeOutput } from "../../../../shared/terminal-model.guard";
import { ErrorOutput } from "./outputs/ErrorOutput";
import { HintOutput } from "./outputs/HintOutput";
import { MediaOutput } from "./outputs/MediaOutput";
import { ProgressOutput } from "./outputs/ProgressOutput";
import { TableOutput } from "./outputs/TableOutput";
import { TextOutput } from "./outputs/TextOutput";

const KNOWN_STATUSES: readonly BlockStatus[] = [
  "pending",
  "running",
  "done",
  "error",
  "cancelled",
];

/** 未知状态按契约降级为 pending（ARCH-005 §7.3）。 */
function normalizeStatus(value: BlockStatus): BlockStatus {
  return (KNOWN_STATUSES as readonly string[]).includes(value)
    ? value
    : "pending";
}

/** 完成耗时：`842ms` / `1.8s`；运行中或非法值返回 null。 */
function formatDuration(durationMs: number | null): string | null {
  if (durationMs === null || !Number.isFinite(durationMs) || durationMs < 0) {
    return null;
  }

  if (durationMs < 1000) {
    return `${Math.round(durationMs)}ms`;
  }

  const seconds = durationMs / 1000;

  return seconds < 10
    ? `${seconds.toFixed(1).replace(/\.0$/, "")}s`
    : `${Math.round(seconds)}s`;
}

function renderOutput(output: Output, key: number) {
  switch (output.type) {
    case "text":
      return <TextOutput key={key} output={output} />;
    case "table":
      return <TableOutput key={key} output={output} />;
    case "progress":
      return <ProgressOutput key={key} output={output} />;
    case "error":
      return <ErrorOutput key={key} output={output} />;
    case "hint":
      return <HintOutput key={key} output={output} />;
    case "media":
      return <MediaOutput key={key} output={output} />;
    default:
      return null;
  }
}

export type BlockItemProps = {
  block: Block;
};

/**
 * 单个命令块：输入行 + 输出区 + 尾注。
 * 只负责呈现，不接 IPC、不持有命令状态（T-0008 负责容器与接线）。
 */
export function BlockItem({ block }: BlockItemProps) {
  const status = normalizeStatus(block.status);
  const duration = formatDuration(block.durationMs);
  const running = status === "pending" || status === "running";
  const statusText = running
    ? status === "pending"
      ? "等待"
      : "运行中"
    : duration ??
      (status === "error" ? "失败" : status === "cancelled" ? "已取消" : "完成");

  return (
    <article
      className="np-blocks-item"
      data-status={status}
      data-block-id={block.id}
    >
      {block.input.trim() ? (
        <div className="np-blocks-item__input">
          <span className="np-blocks-item__prompt" aria-hidden="true">
            ›
          </span>
          <span className="np-blocks-item__command">{block.input}</span>
          <span
            className="np-blocks-item__status"
            title={running ? "状态" : duration ? "耗时" : "状态"}
          >
            <i className="np-blocks-item__status-dot" aria-hidden="true" />
            {statusText}
          </span>
        </div>
      ) : null}

      <div className="np-blocks-item__outputs">
        {block.outputs.map((output, index) =>
          renderOutput(normalizeOutput(output), index),
        )}
      </div>

      {block.footer ? (
        <p className="np-blocks-item__footer">{block.footer}</p>
      ) : null}
    </article>
  );
}
