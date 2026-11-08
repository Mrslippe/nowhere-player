import type { DesignSnapshot } from "./snapshot";
import { FONT_SIZE_TOKENS } from "./token-names";

const SIZE_SAMPLES: Record<(typeof FONT_SIZE_TOKENS)[number], string> = {
  "--fs-10": "10px · 终端状态栏 / 辅助信息",
  "--fs-12": "12px · UI 次要文本",
  "--fs-13": "13px · 正文 / 终端输出",
  "--fs-15": "15px · 区块小标题",
  "--fs-20": "20px · 页面标题",
};

/** 字号阶 + 实际生效的 UI / 等宽字体栈。 */
export function TypeSection({ snapshot }: { snapshot: DesignSnapshot | null }) {
  return (
    <section className="demo__section">
      <h2 className="demo__section-title">字号与字体</h2>
      <p className="demo__section-note">
        UI 走系统栈（Inter 优先），终端走等宽栈；M0 不打包字体文件。
      </p>

      <div className="type-scale">
        {FONT_SIZE_TOKENS.map((token) => (
          <div className="type-scale__row" key={token}>
            <span
              className="type-scale__sample"
              style={{ fontSize: `var(${token})` }}
            >
              Nowhere 播放器 · 0123
            </span>
            <code className="type-scale__label np-mono">{SIZE_SAMPLES[token]}</code>
          </div>
        ))}
      </div>

      <h3 className="demo__subtitle">字体栈（计算值）</h3>
      <dl className="font-stack">
        <dt className="np-mono">--font-ui</dt>
        <dd className="np-mono">{snapshot?.fonts.ui ?? "读取中…"}</dd>
        <dt className="np-mono">--font-mono</dt>
        <dd className="np-mono">{snapshot?.fonts.mono ?? "读取中…"}</dd>
      </dl>

      <p className="demo__section-note np-tabular">
        数字等宽（tabular-nums）：00:00 / 03:47 / 128 kbps
      </p>
    </section>
  );
}
