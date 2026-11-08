import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ProgressOutput as ProgressOutputModel } from "../../../../../shared/terminal-model";
import { ProgressOutput } from "./ProgressOutput";

const OUTPUT: ProgressOutputModel = {
  type: "progress",
  label: "扫描 ~/Music",
  current: 1284,
  total: 1284,
  unit: "首",
};

describe("ProgressOutput", () => {
  it("渲染 label、进度条与 current/total unit", () => {
    const html = renderToStaticMarkup(<ProgressOutput output={OUTPUT} />);

    expect(html).toContain("扫描 ~/Music");
    expect(html).toContain('role="progressbar"');
    expect(html).toContain('aria-valuenow="100"');
    expect(html).toContain("1284/1284 首");
    expect(html).toContain('data-indeterminate="false"');
  });

  it("total = 0 时进入不确定态，只显示已处理数量", () => {
    const html = renderToStaticMarkup(
      <ProgressOutput output={{ ...OUTPUT, current: 42, total: 0 }} />,
    );

    expect(html).toContain('data-indeterminate="true"');
    expect(html).toContain("42 首");
    expect(html).not.toContain("aria-valuenow");
  });

  it("current 超过 total 时进度条仍封顶 100%", () => {
    const html = renderToStaticMarkup(
      <ProgressOutput output={{ ...OUTPUT, current: 2000 }} />,
    );

    expect(html).toContain('aria-valuenow="100"');
    expect(html).toContain("2000/1284 首");
  });
});
