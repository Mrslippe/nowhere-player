import type { HintOutput as HintOutputModel } from "../../../../../shared/terminal-model";

export type HintOutputProps = {
  output: HintOutputModel;
};

/**
 * `hint` 原语：12px muted 小字；有 suggestion 时用 accent 标出（如「按 Tab 采纳」）。
 */
export function HintOutput({ output }: HintOutputProps) {
  return (
    <p className="np-blocks-hint">
      {output.text}
      {output.suggestion ? (
        <>
          {" "}
          <span className="np-blocks-hint__suggestion">{output.suggestion}</span>
        </>
      ) : null}
    </p>
  );
}
