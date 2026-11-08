import { describe, expect, it } from "vitest";
import fixture from "./fixtures/terminal-demo.json";
import { UNSUPPORTED_OUTPUT_TEXT, isBlock, normalizeOutput } from "./terminal-model.guard";
import type {
  ErrorOutput,
  HintOutput,
  MediaOutput,
  ProgressOutput,
  TableOutput,
  TextOutput,
} from "./terminal-model";

describe("terminal-demo.json", () => {
  it("符合 Block 契约并覆盖 6 种 Output", () => {
    if (!isBlock(fixture)) {
      throw new Error("terminal-demo.json 不符合 Block 契约");
    }

    expect(fixture.schemaVersion).toBe(1);
    expect(fixture.status).toBe("done");
    expect(fixture.durationMs).toBe(1842);
    expect(fixture.outputs.map((output) => output.type)).toEqual([
      "text",
      "table",
      "progress",
      "error",
      "hint",
      "media",
    ]);

    const [text, table, progress, error, hint, media] = fixture.outputs as [
      TextOutput,
      TableOutput,
      ProgressOutput,
      ErrorOutput,
      HintOutput,
      MediaOutput,
    ];

    expect(text.spans.some((span) => span.bold)).toBe(true);
    expect(text.spans.some((span) => span.dim)).toBe(true);
    expect(text.spans.some((span) => span.color === "accent")).toBe(true);

    expect(table.columns).toHaveLength(4);
    expect(table.rows).toHaveLength(5);
    expect(table.rows.every((row) => row.length === table.columns.length)).toBe(true);
    expect(table.selectable).toBe(true);

    expect(progress.label).toBe("扫描 ~/Music");
    expect(progress.current).toBe(1284);
    expect(progress.total).toBe(1284);
    expect(progress.unit).toBe("首");

    expect(error.code).toBe("E_LIB_PERMISSION");
    expect(error.hint).toContain("scan --skip-private");

    expect(hint.suggestion).toBe("按 Tab 采纳");

    expect(media.kind).toBe("cover");
    expect(media.ref).toContain("cover.jpg");
    expect(media.size).toBe(48);
  });

  it("未知 Output.type 降级为 text + [unsupported]", () => {
    const output = normalizeOutput({ type: "chart", series: [1, 2, 3] });

    expect(output.type).toBe("text");
    if (output.type !== "text") {
      throw new Error("unreachable");
    }
    expect(output.spans).toHaveLength(1);
    expect(output.spans[0].text).toBe(UNSUPPORTED_OUTPUT_TEXT);
  });
});
