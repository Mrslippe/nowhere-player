import type { TableOutput as TableOutputModel } from "../../../../../shared/terminal-model";

/** 单元格最大字符数（ARCH-005 §3）。 */
const MAX_CELL_CHARS = 40;

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
            {output.columns.map((column, index) => (
              <th key={index} scope="col">
                {column}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {output.rows.map((row, rowIndex) => (
            <tr key={rowIndex}>
              {output.columns.map((column, cellIndex) => {
                const cell = truncateCell(row[cellIndex] ?? "");

                return (
                  <td key={cellIndex} data-label={column} title={cell.title}>
                    {cell.text}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
