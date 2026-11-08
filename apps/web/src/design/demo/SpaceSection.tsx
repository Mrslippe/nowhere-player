import type { DesignSnapshot } from "./snapshot";
import {
  MOTION_TOKENS,
  RADIUS_TOKENS,
  SHADOW_TOKENS,
  SPACE_TOKENS,
} from "./token-names";

/** 间距 / 圆角 / 阴影 / 动效四组结构 token。 */
export function SpaceSection({ snapshot }: { snapshot: DesignSnapshot | null }) {
  return (
    <section className="demo__section">
      <h2 className="demo__section-title">间距 / 圆角 / 阴影 / 动效</h2>
      <p className="demo__section-note">
        结构 token 不随主题变化；进度与尺寸统一用 4pt 网格。
      </p>

      <h3 className="demo__subtitle">间距 --sp-1 … --sp-6</h3>
      <div className="space-list">
        {SPACE_TOKENS.map((token) => (
          <div className="space-row" key={token}>
            <code className="space-row__name np-mono">{token}</code>
            <span className="space-row__bar" style={{ width: `var(${token})` }} />
            <span className="space-row__value np-mono">
              {snapshot?.tokens[token] ?? "…"}
            </span>
          </div>
        ))}
      </div>

      <h3 className="demo__subtitle">圆角 --r-sm … --r-xl</h3>
      <div className="radius-list">
        {RADIUS_TOKENS.map((token) => (
          <div className="radius-item" key={token}>
            <span
              className="radius-item__box"
              style={{ borderRadius: `var(${token})` }}
            />
            <code className="radius-item__label np-mono">
              {token} · {snapshot?.tokens[token] ?? "…"}
            </code>
          </div>
        ))}
      </div>

      <h3 className="demo__subtitle">阴影 --shadow-1 / --shadow-2</h3>
      <div className="shadow-list">
        {SHADOW_TOKENS.map((token) => (
          <div
            className="shadow-card"
            data-shadow={token === "--shadow-1" ? "1" : "2"}
            key={token}
          >
            {token}
          </div>
        ))}
      </div>

      <h3 className="demo__subtitle">动效 --dur-* / --ease-out</h3>
      <div className="motion-list">
        {MOTION_TOKENS.map((token) => (
          <div className="motion-row" key={token}>
            <code className="motion-row__name np-mono">{token}</code>
            <span className="motion-row__value np-mono">
              {snapshot?.tokens[token] ?? "…"}
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}
