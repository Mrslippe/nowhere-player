import type { TableOutput as TableOutputModel } from "../../../../../shared/terminal-model";

/** 单元格最大字符数（ARCH-005 §3）。 */
const MAX_CELL_CHARS = 40;

/** 列宽角色：标题永远吃剩余空间，其余列固定（T-0051 B）。 */
type ColumnRole = "index" | "source" | "title" | "artist" | "duration" | "current";

function columnRole(column: string): ColumnRole | null {
  switch (column) {
    case "#":
    case "序号":
      return "index";
    case "来源":
      return "source";
    case "标题":
    case "曲名":
      return "title";
    case "艺人":
      return "artist";
    case "时长":
      return "duration";
    case "当前":
      return "current";
    default:
      return null;
  }
}

/** 超长单元格截断为 `…` 并把全文放进 `title`。 */
function truncateCell(value: string): { text: string; title: string | undefined } {
  const characters = Array.from(value);

  if (characters.length <= MAX_CELL_CHARS) {
    return { text: value, title: undefined };
  }

  return {
    text: `${characters.slice(0, MAX_CELL_CHARS - 1).join("")}…`,
    title: value,
  };
}

export type TableOutputProps = {
  output: TableOutputModel;
};

/**
 * `table` 原语：真表格，列宽随内容自适应（CSS `table-layout: auto`）。
 * 窄窗口（< 640px）由 blocks.css 降级为「每行一条」的列表形态。
 */
export function TableOutput({ output }: TableOutputProps) {
  return (
    <div className="np-blocks-table-wrap">
      <table
        className="np-blocks-table"
        data-selectable={output.selectable ? "true" : "false"}
        data-columns={output.columns.length}
      >
        <thead>
          <tr>
            {output.columns.map((column, index) => {
              const role = columnRole(column);
              return (
                <th key={index} scope="col" data-role={role ?? undefined}>
                  {column}
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {output.rows.map((row, rowIndex) => {
            // queue list 用「▶ n」标记当前播放行；表格是静态块，不按帧重算。
            const current = (row[0] ?? "").trimStart().startsWith("▶");
            return (
            <tr
              key={rowIndex}
              data-current={current ? "true" : undefined}
              aria-current={current ? "true" : undefined}
            >
              {output.columns.map((column, cellIndex) => {
                const cell = truncateCell(row[cellIndex] ?? "");
                const role = columnRole(column);

                return (
                  <td
                    key={cellIndex}
                    data-label={column}
                    data-role={role ?? undefined}
                    title={cell.title}
                  >
                    {cell.text}
                  </td>
                );
              })}
            </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
