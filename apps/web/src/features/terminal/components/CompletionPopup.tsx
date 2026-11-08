/**
 * Tab 补全弹层（T-0020 §6.3）。
 *
 * 纯展示：候选、高亮由 {@link CommandInput} 决定。最多 {@link POPUP_LIMIT} 条，
 * 每行 = 候选文本 + 类型标签 + 一行说明。
 */
import type { Completion, CompletionItem } from "../../../shared/command-bridge";
import { POPUP_LIMIT } from "../completion";

/** 候选类型的中文标签（与 `np_core::command::CompletionKind` 一一对应）。 */
export const COMPLETION_KIND_LABELS: Record<CompletionItem["kind"], string> = {
  command: "命令",
  subcommand: "子命令",
  enum: "取值",
  dynamic: "序号",
  suggestion: "建议",
};

export interface CompletionPopupProps {
  completion: Completion;
  /** 当前高亮下标（相对可见列表）。 */
  activeIndex: number;
}

export function CompletionPopup({
  completion,
  activeIndex,
}: CompletionPopupProps) {
  const items = completion.items.slice(0, POPUP_LIMIT);

  if (items.length === 0) {
    return null;
  }

  return (
    <div
      className="np-lab__completion"
      role="listbox"
      aria-label="命令补全"
      data-testid="completion-popup"
      data-count={items.length}
    >
      {items.map((item, index) => {
        const active = index === activeIndex;

        return (
          <div
            key={`${item.kind}:${item.text}:${index}`}
            className={`np-lab__completion-item${active ? " is-active" : ""}`}
            role="option"
            aria-selected={active}
            data-active={active ? "true" : "false"}
            data-kind={item.kind}
          >
            <span className="np-lab__completion-text">{item.text}</span>
            <span className="np-lab__completion-kind">
              {COMPLETION_KIND_LABELS[item.kind]}
            </span>
            <span className="np-lab__completion-summary">{item.summary}</span>
          </div>
        );
      })}
    </div>
  );
}
