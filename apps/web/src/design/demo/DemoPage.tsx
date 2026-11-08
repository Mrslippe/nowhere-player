import { Button } from "../components";
import { ComponentsSection } from "./ComponentsSection";
import { PaletteSection } from "./PaletteSection";
import { SpaceSection } from "./SpaceSection";
import { useDesignSnapshot } from "./snapshot";
import type { DesignSnapshot } from "./snapshot";
import { TypeSection } from "./TypeSection";
import { ALL_TOKENS } from "./token-names";
import { THEMES, useTheme } from "./use-theme";

export function DemoPage() {
  const { theme, setTheme } = useTheme();
  const snapshot: DesignSnapshot | null = useDesignSnapshot(theme);

  return (
    <div className="demo">
      <header className="demo__header">
        <div>
          <h1 className="demo__title">Nowhere Player · 设计 Token</h1>
          <p className="demo__subtitle">
            T-0003 开发期入口（不进生产路由）· 当前主题{" "}
            <code className="np-mono">{theme}</code> · 共 {ALL_TOKENS.length} 个
            token
          </p>
        </div>
      </header>

      <PaletteSection snapshot={snapshot} />
      <TypeSection snapshot={snapshot} />
      <SpaceSection snapshot={snapshot} />
      <ComponentsSection snapshot={snapshot} />

      <section className="demo__section">
        <h2 className="demo__section-title">主题切换</h2>
        <p className="demo__section-note">
          三套主题只覆盖颜色变量；切换后背景 / 文本 / 强调色整体变化，也可用{" "}
          <code className="np-mono">?theme=amber</code> 直链。
        </p>
        <div className="demo__toolbar" role="group" aria-label="主题切换">
          <span className="demo__toolbar-label">
            当前：<code className="np-mono">{theme}</code>
          </span>
          {THEMES.map((name) => (
            <Button
              key={name}
              size="sm"
              variant={name === theme ? "primary" : "ghost"}
              aria-pressed={name === theme}
              onClick={() => setTheme(name)}
            >
              {name}
            </Button>
          ))}
        </div>
      </section>

      <footer className="demo__footer">
        色板 / 字号 / 间距 / 圆角 / 阴影 / 动效全部来自 CSS 变量；三套主题：
        graphite（默认）、amber、ice。
      </footer>
    </div>
  );
}
