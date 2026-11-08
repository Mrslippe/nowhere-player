import { Button } from "../../../../../design";
import type { ErrorOutput as ErrorOutputModel } from "../../../../../shared/terminal-model";

export type ErrorOutputProps = {
  output: ErrorOutputModel;
};

/**
 * `error` 原语：左侧 danger 竖线 + code + 消息 + hint + 建议动作。
 * v1 的 `actions` 只展示按钮文案，点击行为由后续命令层约定（ARCH-005 §7.1）。
 */
export function ErrorOutput({ output }: ErrorOutputProps) {
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

      {output.actions && output.actions.length > 0 ? (
        <div className="np-blocks-error__actions">
          {output.actions.map((action, index) => (
            <Button key={`${action}-${index}`} variant="ghost" size="sm">
              {action}
            </Button>
          ))}
        </div>
      ) : null}
    </section>
  );
}
