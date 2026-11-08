/** 实验室三种视图（T-0008 §6.1）。 */

export const LAB_VARIANTS = ["blocks", "xterm", "both"] as const;

export type LabVariant = (typeof LAB_VARIANTS)[number];

/** 默认视图：结构化渲染 + 真实链路（T-0019 起）。 */
export const DEFAULT_VARIANT: LabVariant = "blocks";

export const VARIANT_LABELS: Readonly<Record<LabVariant, string>> = {
  blocks: "结构化",
  xterm: "xterm",
  both: "分屏",
};

export function isLabVariant(value: string | null | undefined): value is LabVariant {
  return value === "blocks" || value === "xterm" || value === "both";
}

/** `?variant=` 非法或缺省时回落到 {@link DEFAULT_VARIANT}。 */
export function parseVariant(value: string | null | undefined): LabVariant {
  return isLabVariant(value) ? value : DEFAULT_VARIANT;
}
