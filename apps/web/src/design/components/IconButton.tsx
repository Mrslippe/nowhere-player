import type { ButtonHTMLAttributes, ReactNode } from "react";

export type IconButtonSize = "sm" | "md";

export type IconButtonProps = Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  "aria-label" | "children"
> & {
  /** 必填：纯图标按钮必须有可读名称。 */
  "aria-label": string;
  /** sm = 20×20（图标 14px），md = 32×32（图标 16px），默认 md。 */
  size?: IconButtonSize;
  children: ReactNode;
};

/** 纯图标按钮：尺寸由 data-size + CSS 变量控制，图标由调用方传入。 */
export function IconButton({
  type = "button",
  size = "md",
  className,
  children,
  ...rest
}: IconButtonProps) {
  const classes = ["np-icon-btn", className].filter(Boolean).join(" ");

  return (
    <button {...rest} type={type} className={classes} data-size={size}>
      {children}
    </button>
  );
}
