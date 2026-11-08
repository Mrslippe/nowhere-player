import { describe, expect, it } from "vitest";
import { isPlayerState, PLAYER_STATES, readPlayerState } from "./state";

describe("readPlayerState", () => {
  it.each(PLAYER_STATES)("%s 可通过 ?state= 读取", (state) => {
    expect(readPlayerState(`?state=${state}`)).toBe(state);
  });

  it("缺失、空值或非法值回退播放态", () => {
    expect(readPlayerState("")).toBe("playing");
    expect(readPlayerState("?state=")).toBe("playing");
    expect(readPlayerState("?state=nope")).toBe("playing");
  });

  it("从完整查询串中取 state", () => {
    expect(readPlayerState("?foo=1&state=paused")).toBe("paused");
  });
});

describe("isPlayerState", () => {
  it("只接受 6 个约定状态名", () => {
    expect(isPlayerState("hover")).toBe(true);
    expect(isPlayerState("HOVER")).toBe(false);
    expect(isPlayerState(null)).toBe(false);
    expect(isPlayerState(3)).toBe(false);
  });
});
