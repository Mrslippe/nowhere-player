import type { ButtonHTMLAttributes, ReactNode } from "react";

export type IconButtonProps = Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  "aria-label" | "children"
> & {
  /** 必填：纯图标按钮必须有可读名称。 */
  "aria-label": string;
  children: ReactNode;
};

/** 32×32 的纯图标按钮，图标由调用方传入（通常为 <Icon size={16} />）。 */
export function IconButton({
  type = "button",
  className,
  children,
  ...rest
}: IconButtonProps) {
  const classes = ["np-icon-btn", className].filter(Boolean).join(" ");

  return (
    <button {...rest} type={type} className={classes}>
      {children}
    </button>
  );
}
