export type SpinnerSize = 12 | 16;

export type SpinnerProps = {
  size?: SpinnerSize;
  /** 供读屏使用的说明。 */
  label?: string;
  className?: string;
};

/**
 * 加载指示器。prefers-reduced-motion: reduce 时由 CSS 停止旋转，保留静态圆弧。
 */
export function Spinner({
  size = 16,
  label = "加载中",
  className,
}: SpinnerProps) {
  const classes = ["np-spinner", className].filter(Boolean).join(" ");

  return (
    <svg
      className={classes}
      data-size={size}
      width={size}
      height={size}
      viewBox="0 0 16 16"
      role="status"
      aria-label={label}
      focusable="false"
    >
      <circle
        className="np-spinner__track"
        cx="8"
        cy="8"
        r="6.4"
        fill="none"
        strokeWidth="2"
      />
      <circle
        className="np-spinner__arc"
        cx="8"
        cy="8"
        r="6.4"
        fill="none"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}
