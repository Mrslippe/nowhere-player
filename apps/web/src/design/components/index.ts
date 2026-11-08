/*
 * 基础组件统一出口：import 一次即带上组件样式。
 * 组件样式也由这里引入，避免每个使用方各写一遍 CSS 路径。
 */
import "./components.css";

export { Button } from "./Button";
export type { ButtonProps, ButtonSize, ButtonVariant } from "./Button";

export { IconButton } from "./IconButton";
export type { IconButtonProps } from "./IconButton";

export { ProgressBar } from "./ProgressBar";
export type { ProgressBarProps } from "./ProgressBar";

export { Spinner } from "./Spinner";
export type { SpinnerProps, SpinnerSize } from "./Spinner";

export { Kbd } from "./Kbd";
export type { KbdProps } from "./Kbd";

export { Icon } from "./Icon";
export type { IconName, IconProps } from "./Icon";
