import type { TextOutput as TextOutputModel } from "../../../../../shared/terminal-model";

export type TextOutputProps = {
  output: TextOutputModel;
};

/**
 * `text` 原语：按 `spans` 逐段渲染行内样式（ARCH-005 §2.3）。
 * `tone` 是整段默认色，单个 span 的 `color` 覆盖它。
 */
export function TextOutput({ output }: TextOutputProps) {
  return (
    <p className="np-blocks-text" data-tone={output.tone ?? undefined}>
      {output.spans.map((span, index) => (
        <span
          key={index}
          className="np-blocks-text__span"
          data-color={span.color ?? undefined}
          data-bold={span.bold ? "true" : undefined}
          data-dim={span.dim ? "true" : undefined}
          data-italic={span.italic ? "true" : undefined}
        >
          {span.text}
        </span>
      ))}
    </p>
  );
}
