import { Button } from "../../../../../design";
import type {
  BlockAction,
  ErrorOutput as ErrorOutputModel,
} from "../../../../../shared/terminal-model";
import { runBlockAction } from "../../../block-actions";

export type ErrorOutputProps = {
  output: ErrorOutputModel;
  /** 块级 actions；由 BlockItem 用 readBlockActions 校验后传入。 */
  actions?: BlockAction[];
  /** 与输入框同一个 submit → cmd.exec 路径。 */
  onAction?: (input: string) => void;
};

/**
 * `error` 原语：错误卡 + code/消息/hint + 块级可执行动作（ARCH-005 §8）。
 *
 * `Output::Error.actions` 是 legacy 字段，UI 不再渲染；动作只认块级
 * `Block.actions`。按钮是原生 button：Tab 可聚焦、Enter/Space 触发、title
 * 显示将执行的命令。
 */
export function ErrorOutput({ output, actions, onAction }: ErrorOutputProps) {
  return (
    <section
      className="np-blocks-error"
      data-code={output.code}
      aria-label={`错误 ${output.code}`}
    >
      <div className="np-blocks-error__head">
        <span className="np-blocks-error__code">{output.code}</span>
        <span className="np-blocks-error__message">{output.message}</span>
      </div>

      {output.hint ? (
        <p className="np-blocks-error__hint">{output.hint}</p>
      ) : null}

      {actions && actions.length > 0 ? (
        <div className="np-blocks-error__actions">
          {actions.map((action, index) => (
            <Button
              key={`${action.label}-${index}`}
              type="button"
              variant={index === 0 ? "primary" : "ghost"}
              size="sm"
              title={action.input}
              disabled={onAction === undefined}
              onClick={() => {
                runBlockAction(action, onAction);
              }}
            >
              {action.label}
            </Button>
          ))}
        </div>
      ) : null}
    </section>
  );
}
