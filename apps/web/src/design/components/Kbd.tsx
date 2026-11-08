import type { HTMLAttributes } from "react";

export type KbdProps = HTMLAttributes<HTMLElement>;

/** 键帽：等宽字体 + 描边 + 底部加厚 1px，用于终端状态栏与快捷键提示。 */
export function Kbd({ className, children, ...rest }: KbdProps) {
  const classes = ["np-kbd", className].filter(Boolean).join(" ");

  return (
    <kbd {...rest} className={classes}>
      {children}
    </kbd>
  );
}
