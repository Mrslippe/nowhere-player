import type { Block } from "../../../../shared/terminal-model";
import { BlockItem } from "./BlockItem";
import "./blocks.css";

export type BlocksViewProps = {
  blocks: Block[];
  className?: string;
};

/**
 * 结构化块渲染器入口：纯受控，把 `Block[]` 依次画出来。
 * 不负责输入、滚动、贴底与命令执行——那些属于 T-0008。
 */
export function BlocksView({ blocks, className }: BlocksViewProps) {
  const classes = ["np-blocks", className].filter(Boolean).join(" ");

  return (
    <section className={classes} aria-label="终端输出">
      {blocks.map((block) => (
        <BlockItem key={block.id} block={block} />
      ))}
    </section>
  );
}
