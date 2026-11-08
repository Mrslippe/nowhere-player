import type { IconName } from "../components";
import { Button, Icon, IconButton, Kbd, ProgressBar, Spinner } from "../components";
import type { DesignSnapshot } from "./snapshot";

const ICON_NAMES: IconName[] = [
  "play",
  "pause",
  "skip-back",
  "skip-forward",
  "chevron-right",
  "check",
  "copy",
  "alert-circle",
];

const PLACEHOLDER = "读取中…";

/** 六个基础组件的陈列区 + 动效降级的运行时检查。 */
export function ComponentsSection({
  snapshot,
}: {
  snapshot: DesignSnapshot | null;
}) {
  const motion = snapshot?.motion ?? {
    reduced: false,
    spinnerAnimation: PLACEHOLDER,
    buttonTransition: PLACEHOLDER,
  };

  return (
    <section className="demo__section">
      <h2 className="demo__section-title">组件陈列</h2>
      <p className="demo__section-note">
        Button / IconButton / ProgressBar / Spinner / Kbd / Icon，各一个基础变体，
        供 T-0004 / T-0006 / T-0007 复用。
      </p>

      <div className="component-grid">
        <div className="component-cell">
          <h3 className="component-cell__title">Button</h3>
          <div className="component-row">
            <Button variant="primary">播放</Button>
            <Button variant="ghost">加入队列</Button>
            <Button variant="danger">移除</Button>
          </div>
          <div className="component-row">
            <Button size="sm" variant="primary">
              sm primary
            </Button>
            <Button size="sm" variant="ghost">
              sm ghost
            </Button>
            <Button variant="primary" disabled>
              禁用
            </Button>
          </div>
        </div>

        <div className="component-cell">
          <h3 className="component-cell__title">IconButton · 32×32</h3>
          <div className="component-row">
            <IconButton aria-label="上一首">
              <Icon name="skip-back" />
            </IconButton>
            <IconButton aria-label="播放">
              <Icon name="play" />
            </IconButton>
            <IconButton aria-label="暂停">
              <Icon name="pause" />
            </IconButton>
            <IconButton aria-label="下一首">
              <Icon name="skip-forward" />
            </IconButton>
            <IconButton aria-label="复制命令">
              <Icon name="copy" />
            </IconButton>
          </div>
        </div>

        <div className="component-cell">
          <h3 className="component-cell__title">ProgressBar</h3>
          <div className="progress-stack">
            <code className="component-cell__label np-mono">2px · 37%</code>
            <ProgressBar value={0.37} size={2} label="播放进度 37%" />
            <code className="component-cell__label np-mono">4px · 68%</code>
            <ProgressBar value={0.68} size={4} label="播放进度 68%" />
            <code className="component-cell__label np-mono">不确定态</code>
            <ProgressBar size={2} indeterminate label="扫描中" />
          </div>
        </div>

        <div className="component-cell">
          <h3 className="component-cell__title">Spinner</h3>
          <div className="component-row">
            <Spinner size={16} label="加载中" />
            <Spinner size={12} label="加载中" />
            <span className="component-cell__label np-mono">16px / 12px</span>
          </div>
        </div>

        <div className="component-cell">
          <h3 className="component-cell__title">Kbd</h3>
          <div className="kbd-row">
            <Kbd>Ctrl</Kbd>
            <span>+</span>
            <Kbd>K</Kbd>
            <span className="kbd-row__text">命令面板</span>
          </div>
          <div className="kbd-row">
            <Kbd>Esc</Kbd>
            <span className="kbd-row__text">取消</span>
            <Kbd>Enter</Kbd>
            <span className="kbd-row__text">执行</span>
          </div>
        </div>

        <div className="component-cell">
          <h3 className="component-cell__title">Icon · 8 个</h3>
          <div className="icon-grid">
            {ICON_NAMES.map((name) => (
              <div className="icon-cell" key={name}>
                <Icon name={name} label={name} />
                <code className="icon-cell__name np-mono">{name}</code>
              </div>
            ))}
          </div>
        </div>
      </div>

      <div
        className="motion-probe"
        data-motion-pref={motion.reduced ? "reduce" : "no-preference"}
        data-spinner-animation={motion.spinnerAnimation}
        data-button-transition={motion.buttonTransition}
      >
        <span className="motion-probe__title">运行时动效检查</span>
        <span>
          prefers-reduced-motion：
          {motion.reduced ? "reduce（动画已停）" : "no-preference（动画开启）"}
        </span>
        <span className="motion-probe__value np-mono">
          Spinner animation-name: {motion.spinnerAnimation} · Button
          transition-duration: {motion.buttonTransition}
        </span>
      </div>
    </section>
  );
}
