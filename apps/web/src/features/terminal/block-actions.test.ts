import { describe, expect, it, vi } from "vitest";

import { runBlockAction } from "./block-actions";

describe("runBlockAction", () => {
  it("把动作 input 原样交给同一个 submit，并返回实际提交值", () => {
    const submit = vi.fn();
    const input = runBlockAction(
      { label: "只用本地搜索", input: "  search 晴天 --local  " },
      submit,
    );

    expect(input).toBe("search 晴天 --local");
    expect(submit).toHaveBeenCalledTimes(1);
    expect(submit).toHaveBeenCalledWith("search 晴天 --local");
  });

  it("空 input 或无回调时不触发", () => {
    const submit = vi.fn();

    expect(runBlockAction({ label: "重试", input: "  " }, submit)).toBeNull();
    expect(runBlockAction({ label: "重试", input: "perm" }, undefined)).toBeNull();
    expect(submit).not.toHaveBeenCalled();
  });
});
