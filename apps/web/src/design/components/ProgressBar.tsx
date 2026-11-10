import type { CSSProperties } from "react";

export type ProgressBarProps = {
  /** 0–1 的完成度；不确定态下忽略。 */
  value?: number;
  /** 轨道高度：2px / 3px / 4px。 */
  size?: 2 | 3 | 4;
  /** 不确定态：来回扫动，不表达具体进度。 */
  indeterminate?: boolean;
  /** 供读屏使用的说明，调用方应填写（如「扫描进度」）。 */
  label?: string;
  className?: string;
};

/**
 * 进度条。进度只通过 transform: scaleX() 表达，不写 width，避免触发重排。
 */
export function ProgressBar({
  value = 0,
  size = 2,
  indeterminate = false,
  label,
  className,
}: ProgressBarProps) {
  const clamped = Number.isFinite(value)
    ? Math.min(1, Math.max(0, value))
    : 0;
  const classes = ["np-progress", className].filter(Boolean).join(" ");
  const style = indeterminate
    ? undefined
    : ({ "--np-progress": String(clamped) } as CSSProperties);

  return (
    <div
      className={classes}
      data-size={size}
      data-indeterminate={indeterminate}
      style={style}
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={indeterminate ? undefined : Math.round(clamped * 100)}
    >
      <div className="np-progress__indicator" />
    </div>
  );
}
