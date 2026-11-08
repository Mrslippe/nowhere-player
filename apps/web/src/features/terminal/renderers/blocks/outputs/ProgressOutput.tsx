import { ProgressBar } from "../../../../../design";
import type { ProgressOutput as ProgressOutputModel } from "../../../../../shared/terminal-model";

/** `current/total unit`；总量未知时只显示 `current unit`。 */
function countText(current: number, total: number, unit: string): string {
  const suffix = unit ? ` ${unit}` : "";
  return total > 0 ? `${current}/${total}${suffix}` : `${current}${suffix}`;
}

export type ProgressOutputProps = {
  output: ProgressOutputModel;
};

/**
 * `progress` 原语：单行 label + 条 + 计数。
 * `total = 0` 表示总量未知，交给 ProgressBar 的不确定态（ARCH-005 §2.2）。
 */
export function ProgressOutput({ output }: ProgressOutputProps) {
  const current = Number.isFinite(output.current) ? output.current : 0;
  const total =
    Number.isFinite(output.total) && output.total > 0 ? output.total : 0;
  const indeterminate = total === 0;
  const value = indeterminate ? 0 : Math.min(1, Math.max(0, current / total));

  return (
    <div
      className="np-blocks-progress"
      data-indeterminate={indeterminate ? "true" : "false"}
    >
      <span className="np-blocks-progress__label">{output.label}</span>
      <ProgressBar
        className="np-blocks-progress__bar"
        value={value}
        indeterminate={indeterminate}
        label={`${output.label} 进度`}
      />
      <span className="np-blocks-progress__value">
        {countText(current, total, output.unit)}
      </span>
    </div>
  );
}
