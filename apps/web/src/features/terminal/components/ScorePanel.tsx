import { Button } from "../../../design";
import {
  compareScores,
  formatScore,
  SCORE_DIMENSIONS,
  SCORE_MAX,
  SCORE_MIN,
  SCORE_SIDES,
  SIDE_KEYS,
  SIDE_LABELS,
  type ComparisonScores,
  type DimensionId,
  type ScoreSide,
} from "./scoring";

const SCORE_VALUES = Array.from(
  { length: SCORE_MAX - SCORE_MIN + 1 },
  (_, index) => SCORE_MIN + index,
);

export interface ScorePanelProps {
  scores: ComparisonScores;
  onChange: (side: ScoreSide, dimension: DimensionId, value: number) => void;
  onExport: () => void;
  collapsed: boolean;
  onToggle: () => void;
  feedback: string | null;
}

function leaderText(scores: ComparisonScores): string {
  const { leader, margin } = compareScores(scores);

  if (leader === "tie") {
    return "当前持平";
  }

  return `${SIDE_LABELS[leader]} 领先 ${formatScore(margin)} 分`;
}

/**
 * 五维评分面板（T-0008 §6.5）。
 *
 * 两套渲染器各打一套 1–5 分，共用同一份权重，才能算出“当前领先方”；
 * 权重来自 `components/scoring.ts`，与 M0 任务清单 §3 对齐。
 */
export function ScorePanel({
  scores,
  onChange,
  onExport,
  collapsed,
  onToggle,
  feedback,
}: ScorePanelProps) {
  const { totals } = compareScores(scores);

  return (
    <aside
      className="np-score"
      data-collapsed={collapsed ? "true" : "false"}
      aria-label="对比评分"
    >
      <div className="np-score__head">
        <button
          type="button"
          className="np-score__toggle"
          aria-expanded={!collapsed}
          onClick={onToggle}
        >
          <span className="np-score__chevron" aria-hidden="true">
            {collapsed ? "▸" : "▾"}
          </span>
          评分面板
        </button>

        <span className="np-score__summary" aria-live="polite">
          <span className="np-score__total" data-side="blocks">
            {SIDE_KEYS.blocks} {formatScore(totals.blocks)}
          </span>
          <span className="np-score__total" data-side="xterm">
            {SIDE_KEYS.xterm} {formatScore(totals.xterm)}
          </span>
          <span className="np-score__leader">{leaderText(scores)}</span>
        </span>

        <span className="np-score__feedback" role="status">
          {feedback ?? ""}
        </span>

        <Button
          variant="ghost"
          size="sm"
          className="np-score__export"
          onClick={onExport}
        >
          导出 JSON
        </Button>
      </div>

      {collapsed ? null : (
        <div className="np-score__body">
          <table className="np-score__table">
            <thead>
              <tr>
                <th scope="col">维度</th>
                <th scope="col">权重</th>
                <th scope="col">{SIDE_LABELS.blocks}</th>
                <th scope="col">{SIDE_LABELS.xterm}</th>
              </tr>
            </thead>
            <tbody>
              {SCORE_DIMENSIONS.map((dimension) => (
                <tr key={dimension.id}>
                  <th scope="row">{dimension.label}</th>
                  <td className="np-score__weight">{dimension.weight}%</td>
                  {SCORE_SIDES.map((side) => (
                    <td key={side}>
                      <div
                        className="np-score__buttons"
                        role="group"
                        aria-label={`${dimension.label} · ${SIDE_LABELS[side]}`}
                      >
                        {SCORE_VALUES.map((value) => (
                          <button
                            key={value}
                            type="button"
                            className="np-score__score"
                            aria-pressed={scores[side][dimension.id] === value}
                            aria-label={`${dimension.label} ${SIDE_LABELS[side]} ${value} 分`}
                            onClick={() => onChange(side, dimension.id, value)}
                          >
                            {value}
                          </button>
                        ))}
                      </div>
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr>
                <th scope="row">加权总分</th>
                <td className="np-score__weight">100%</td>
                <td className="np-score__total">
                  {SIDE_KEYS.blocks} {formatScore(totals.blocks)}
                </td>
                <td className="np-score__total">
                  {SIDE_KEYS.xterm} {formatScore(totals.xterm)}
                </td>
              </tr>
            </tfoot>
          </table>
          <p className="np-score__note">
            每项 {SCORE_MIN}–{SCORE_MAX} 分；权重按 M0 任务清单 §3（30 / 25 / 20 / 15 / 10），
            总分 = Σ(得分 × 权重) ÷ 100。
          </p>
        </div>
      )}
    </aside>
  );
}
