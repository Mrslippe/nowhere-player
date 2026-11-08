import {
  Check,
  ChevronRight,
  CircleExclamationPoint,
  Copy,
  Pause,
  Play,
  SkipBack,
  SkipForward,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";

export type IconName =
  | "play"
  | "pause"
  | "skip-back"
  | "skip-forward"
  | "chevron-right"
  | "check"
  | "copy"
  | "alert-circle";

/*
 * 只允许上面这种具名导入 + 静态映射：禁止 import *、禁止按名字符串动态拼属性，
 * 否则打包会带上整个图标库。
 *
 * 注：lucide-react 1.x 把 AlertCircle 更名为 CircleExclamationPoint，
 * 这里保留任务卡约定的公共名 alert-circle。
 */
const GLYPHS = {
  play: Play,
  pause: Pause,
  "skip-back": SkipBack,
  "skip-forward": SkipForward,
  "chevron-right": ChevronRight,
  check: Check,
  copy: Copy,
  "alert-circle": CircleExclamationPoint,
} satisfies Record<IconName, LucideIcon>;

export type IconProps = {
  name: IconName;
  size?: number;
  strokeWidth?: number;
  /** 有语义时传 label（role="img"）；纯装饰时留空（aria-hidden）。 */
  label?: string;
  className?: string;
};

/** 基础图标：统一尺寸/线宽/可访问性，颜色继承 currentColor。 */
export function Icon({
  name,
  size = 16,
  strokeWidth = 1.75,
  label,
  className,
}: IconProps) {
  const Glyph = GLYPHS[name];
  const classes = ["np-icon", className].filter(Boolean).join(" ");

  return (
    <Glyph
      className={classes}
      size={size}
      strokeWidth={strokeWidth}
      aria-hidden={label ? undefined : true}
      role={label ? "img" : undefined}
      aria-label={label}
    />
  );
}
