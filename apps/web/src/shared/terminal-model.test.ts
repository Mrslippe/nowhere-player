import { describe, expect, it } from "vitest";
import fixture from "./fixtures/terminal-demo.json";
import { UNSUPPORTED_OUTPUT_TEXT, isBlock, normalizeOutput, readBlockActions } from "./terminal-model.guard";
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

  it("未知 media.kind 降级为 cover 而非整条丢弃", () => {
    const output = normalizeOutput({
      type: "media",
      kind: "poster",
      ref: "np://cover/1",
      size: 48,
    });

    expect(output.type).toBe("media");
    if (output.type !== "media") {
      throw new Error("unreachable");
    }
    expect(output.kind).toBe("cover");
  });
});

describe("Block.actions（ARCH-005 §8）", () => {
  it("fixture 的块级动作可读取，旧 Output::Error.actions 不参与", () => {
    expect(readBlockActions(fixture.actions)).toEqual([
      { label: "重试", input: "scan ~/Music --deep" },
    ]);
    const error = fixture.outputs.find((output) => output.type === "error");
    expect(error).toMatchObject({ type: "error", actions: null });
  });

  it("合法动作保留 label/input", () => {
    expect(
      readBlockActions([
        { label: "重试", input: "perm" },
        { label: "只用本地搜索", input: "search 晴天 --local" },
      ]),
    ).toEqual([
      { label: "重试", input: "perm" },
      { label: "只用本地搜索", input: "search 晴天 --local" },
    ]);
  });

  it("非数组 / 元素缺字段 / 空字符串：整组降级为空，但块本体仍合法", () => {
    for (const value of [
      "retry",
      [{ label: "重试" }],
      [{ label: "", input: "perm" }],
      [{ label: "重试", input: "  " }],
      [{ label: "重试", input: "perm", extra: 1 }, null],
    ]) {
      expect(readBlockActions(value)).toBeUndefined();
      expect(
        isBlock({
          ...fixture,
          actions: value,
        }),
      ).toBe(true);
    }
  });
});
