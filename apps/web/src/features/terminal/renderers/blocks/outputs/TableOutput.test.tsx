import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { TableOutput as TableOutputModel } from "../../../../../shared/terminal-model";
import { TableOutput } from "./TableOutput";

/** 42 个字符，刚好超过 40 的上限。 */
const LONG_CELL = "晴".repeat(38) + "ABCD";
const TRUNCATED_CELL = `${Array.from(LONG_CELL).slice(0, 39).join("")}…`;

const OUTPUT: TableOutputModel = {
  type: "table",
  columns: ["#", "曲名", "备注"],
  rows: [
    ["1", "晴天", "正常"],
    ["2", LONG_CELL, ""],
  ],
  selectable: true,
};

describe("TableOutput", () => {
  it("渲染表头与行，selectable 时带 hover 标记", () => {
    const html = renderToStaticMarkup(<TableOutput output={OUTPUT} />);

    expect(html).toContain("<table");
    expect(html).toContain('data-selectable="true"');
    expect(html).toContain('data-columns="3"');
    expect(html).toContain('scope="col"');
    expect(html).toContain("曲名");
    expect(html).toContain("晴天");
    expect(html).toContain('data-label="备注"');
    expect(html).toContain('data-role="index"');
    expect(html).toContain('data-role="title"');
  });

  it("search/queue/playlist show 的共同角色：标题吃剩余、艺人固定", () => {
    const html = renderToStaticMarkup(
      <TableOutput
        output={{
          type: "table",
          columns: ["序号", "来源", "标题", "艺人", "时长"],
          rows: [["1", "本地", "晴天", "周杰伦", "4:29"]],
          selectable: true,
        }}
      />,
    );

    expect(html).toContain('data-role="source"');
    expect(html).toContain('data-role="title"');
    expect(html).toContain('data-role="artist"');
    expect(html).toContain('data-role="duration"');
  });

  it("超过 40 字符的单元格显示 …，title 保留全文", () => {
    const html = renderToStaticMarkup(<TableOutput output={OUTPUT} />);

    expect(html).toContain(TRUNCATED_CELL);
    expect(html).toContain(`title="${LONG_CELL}"`);
    expect(html).not.toContain(`>${LONG_CELL}<`);
  });

  it("不超过 40 字符的单元格不截断、不写 title", () => {
    const html = renderToStaticMarkup(<TableOutput output={OUTPUT} />);

    expect(html).toContain(">晴天<");
    expect(html).not.toContain('title="晴天"');
  });
});
