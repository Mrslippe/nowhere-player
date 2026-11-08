import { describe, expect, it } from "vitest";
import { DEFAULT_WINDOW_LABEL, resolveWindowLabel } from "./window-label";

describe("resolveWindowLabel", () => {
  it("不在 Tauri 运行时回退到 player", () => {
    expect(DEFAULT_WINDOW_LABEL).toBe("player");
    expect(resolveWindowLabel()).toBe("player");
  });
});
