import type { DesignSnapshot } from "./snapshot";
import { COLOR_TOKENS } from "./token-names";

const PLACEHOLDER = "…";

/** 色板 + 当前主题下的 WCAG 对比度实测。 */
export function PaletteSection({ snapshot }: { snapshot: DesignSnapshot | null }) {
  const rows = snapshot?.contrast ?? [];

  return (
    <section className="demo__section">
      <h2 className="demo__section-title">色板</h2>
      <p className="demo__section-note">
        色值定义在 tokens.css（缺省）与 themes.css（三套主题）；下面是当前主题
        <code className="np-mono"> data-theme </code>
        下的计算值，非硬编码。
      </p>

      <div className="palette">
        {COLOR_TOKENS.map((token) => (
          <div className="palette__item" key={token}>
            <span className="palette__chip">
              <span
                className="palette__fill"
                style={{ background: `var(${token})` }}
              />
            </span>
            <code className="palette__token np-mono">{token}</code>
            <span className="palette__value np-mono np-tabular">
              {snapshot?.tokens[token] ?? PLACEHOLDER}
            </span>
          </div>
        ))}
      </div>

      <h3 className="demo__subtitle">对比度实测（正文目标 ≥ 4.5:1）</h3>
      <table className="contrast">
        <thead>
          <tr>
            <th>用途</th>
            <th>前景 / 背景</th>
            <th>对比度</th>
            <th>结论</th>
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td colSpan={4}>读取中{PLACEHOLDER}</td>
            </tr>
          ) : (
            rows.map((row) => (
              <tr key={row.label}>
                <td>{row.label}</td>
                <td className="np-mono">
                  {row.foreground} / {row.background}
                </td>
                <td className="np-mono np-tabular">
                  {row.ratio === null ? PLACEHOLDER : `${row.ratio.toFixed(2)}:1`}
                </td>
                <td>
                  {row.ratio === null
                    ? PLACEHOLDER
                    : row.ratio >= 4.5
                      ? "通过"
                      : "未达标"}
                </td>
              </tr>
            ))
          )}
        </tbody>
      </table>
    </section>
  );
}
