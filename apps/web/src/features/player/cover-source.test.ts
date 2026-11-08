import { describe, expect, it, vi } from "vitest";

import { createCoverSource, isCoverKey } from "./cover-source";
import type { CoverSourceDeps } from "./cover-source";

const KEY = "0123456789abcdef";
const OTHER_KEY = "fedcba9876543210";
const COVER_PATH = "C:\\Users\\demo\\AppData\\Roaming\\com.nowhere.player\\covers\\0123456789abcdef.jpg";
const COVER_URL = "http://asset.localhost/C%3A%5Ccover.jpg";

type InvokeArgs = Record<string, unknown> | undefined;

function fakeDeps(
  handler: (command: string, args: InvokeArgs) => Promise<unknown>,
): { deps: CoverSourceDeps; calls: Array<[string, InvokeArgs]>; convert: ReturnType<typeof vi.fn> } {
  const calls: Array<[string, InvokeArgs]> = [];
  const convert = vi.fn((path: string) => `${COVER_URL}#${path}`);
  const deps: CoverSourceDeps = {
    invoke: (command, args) => {
      calls.push([command, args]);

      return handler(command, args);
    },
    convertFileSrc: convert,
  };

  return { deps, calls, convert };
}

describe("isCoverKey", () => {
  it("只接受 16 位小写十六进制", () => {
    expect(isCoverKey(KEY)).toBe(true);
    expect(isCoverKey("0000000000000000")).toBe(true);

    for (const value of [
      "",
      "..",
      "../../etc/passwd",
      "0123456789abcde/",
      "0123456789abcde\\",
      "0123456789ABCDEF",
      "0123456789abcde",
      "0123456789abcdef0",
      null,
      undefined,
      42,
    ]) {
      expect(isCoverKey(value)).toBe(false);
    }
  });
});

describe("coverSource 的 URL 构建", () => {
  it("合法 key → library.coverUrl → convertFileSrc", async () => {
    const { deps, calls, convert } = fakeDeps(async (command) =>
      command === "library.coverUrl" ? COVER_PATH : null,
    );
    const source = createCoverSource(deps);

    const url = await source.coverUrlForCoverKey(KEY);

    expect(url).toBe(`${COVER_URL}#${COVER_PATH}`);
    expect(calls).toEqual([["library.coverUrl", { key: KEY }]]);
    expect(convert).toHaveBeenCalledWith(COVER_PATH);
  });

  it("同一 key 只请求一次（并发也去重）", async () => {
    const { deps, calls } = fakeDeps(async () => COVER_PATH);
    const source = createCoverSource(deps);

    const [first, second] = await Promise.all([
      source.coverUrlForCoverKey(KEY),
      source.coverUrlForCoverKey(KEY),
    ]);
    const third = await source.coverUrlForCoverKey(KEY);

    expect(first).toBe(third);
    expect(second).toBe(third);
    expect(calls).toHaveLength(1);
  });

  it("非法 key 直接回 null，不发 IPC", async () => {
    const { deps, calls } = fakeDeps(async () => COVER_PATH);
    const source = createCoverSource(deps);

    for (const key of ["", "..", "../x", "0123456789ABCDEF", "0123456789abcde", null]) {
      expect(await source.coverUrlForCoverKey(key)).toBeNull();
    }

    expect(calls).toHaveLength(0);
  });

  it("请求失败缓存为 null，同一 key 不再重试", async () => {
    const { deps, calls } = fakeDeps(async () => {
      throw new Error("E_CMD_ARG");
    });
    const source = createCoverSource(deps);

    expect(await source.coverUrlForCoverKey(KEY)).toBeNull();
    expect(await source.coverUrlForCoverKey(KEY)).toBeNull();
    expect(calls).toHaveLength(1);
  });

  it("coverUrl 返回空串 / 非字符串时回 null", async () => {
    for (const value of ["", null, 42, { path: COVER_PATH }]) {
      const { deps } = fakeDeps(async () => value);
      const source = createCoverSource(deps);

      expect(await source.coverUrlForCoverKey(KEY)).toBeNull();
    }
  });

  it("convertFileSrc 抛错时回 null，且失败被缓存", async () => {
    const { deps, calls } = fakeDeps(async () => COVER_PATH);
    const source = createCoverSource({
      ...deps,
      convertFileSrc: () => {
        throw new Error("bad path");
      },
    });

    expect(await source.coverUrlForCoverKey(KEY)).toBeNull();
    expect(await source.coverUrlForCoverKey(KEY)).toBeNull();
    expect(calls).toHaveLength(1);
  });
});

describe("coverSource 的 trackByPath 接线", () => {
  it("路径 → Track.coverKey → URL", async () => {
    const { deps, calls } = fakeDeps(async (command) => {
      if (command === "library.trackByPath") {
        return { path: "/music/a.wav", coverKey: KEY };
      }

      return COVER_PATH;
    });
    const source = createCoverSource(deps);

    const url = await source.coverUrlForTrack("/music/a.wav");

    expect(url).toBe(`${COVER_URL}#${COVER_PATH}`);
    expect(calls).toEqual([
      ["library.trackByPath", { path: "/music/a.wav" }],
      ["library.coverUrl", { key: KEY }],
    ]);
  });

  it("库中查不到路径 → null，不再请求封面", async () => {
    const { deps, calls } = fakeDeps(async () => null);
    const source = createCoverSource(deps);

    expect(await source.coverUrlForTrack("/music/missing.wav")).toBeNull();
    expect(calls).toEqual([["library.trackByPath", { path: "/music/missing.wav" }]]);
  });

  it("同一路径只查一次库（失败也缓存）", async () => {
    const { deps, calls } = fakeDeps(async () => {
      throw new Error("E_LIB_DB");
    });
    const source = createCoverSource(deps);

    expect(await source.coverUrlForTrack("/music/a.wav")).toBeNull();
    expect(await source.coverUrlForTrack("/music/a.wav")).toBeNull();
    expect(calls).toHaveLength(1);
  });

  it("Track.coverKey 缺失或形状不对时回 null，不发封面 IPC", async () => {
    for (const track of [null, {}, { coverKey: null }, { coverKey: "BAD" }, { coverKey: 7 }]) {
      const { deps, calls } = fakeDeps(async () => track);
      const source = createCoverSource(deps);

      expect(await source.coverUrlForTrack("/music/a.wav")).toBeNull();
      expect(calls).toHaveLength(1);
      expect(calls[0][0]).toBe("library.trackByPath");
    }
  });

  it("空路径 / 非字符串不发 IPC", async () => {
    const { deps, calls } = fakeDeps(async () => null);
    const source = createCoverSource(deps);

    expect(await source.coverUrlForTrack("")).toBeNull();
    expect(await source.coverUrlForTrack(null)).toBeNull();
    expect(await source.coverUrlForTrack(undefined)).toBeNull();
    expect(calls).toHaveLength(0);
  });

  it("不同 key 各自缓存；reset 后重新请求", async () => {
    const { deps, calls } = fakeDeps(async (command) =>
      command === "library.coverUrl" ? COVER_PATH : null,
    );
    const source = createCoverSource(deps);

    await source.coverUrlForCoverKey(KEY);
    await source.coverUrlForCoverKey(OTHER_KEY);
    expect(calls).toHaveLength(2);

    source.reset();
    await source.coverUrlForCoverKey(KEY);
    expect(calls).toHaveLength(3);
  });

  it("trackForPath 返回整条 Track，同一路径并发也只请求一次", async () => {
    const track = {
      path: "/music/a.wav",
      title: "Song",
      artist: "Artist",
      album: "Album",
      coverKey: KEY,
    };
    const { deps, calls } = fakeDeps(async (command) =>
      command === "library.trackByPath" ? track : COVER_PATH,
    );
    const source = createCoverSource(deps);

    const [first, second] = await Promise.all([
      source.trackForPath("/music/a.wav"),
      source.trackForPath("/music/a.wav"),
    ]);

    expect(first).toBe(track);
    expect(second).toBe(track);
    expect(calls).toEqual([["library.trackByPath", { path: "/music/a.wav" }]]);

    // 元数据与封面共用同一份 trackByPath 缓存：只再请求一次 coverUrl。
    expect(await source.coverUrlForTrack("/music/a.wav")).not.toBeNull();
    expect(calls).toHaveLength(2);
    expect(calls[1][0]).toBe("library.coverUrl");
  });

  it("trackForPath 对非对象返回 null，并缓存", async () => {
    for (const value of [null, "track", 7, undefined]) {
      const { deps, calls } = fakeDeps(async () => value);
      const source = createCoverSource(deps);

      expect(await source.trackForPath("/music/a.wav")).toBeNull();
      expect(await source.trackForPath("/music/a.wav")).toBeNull();
      expect(calls).toHaveLength(1);
    }
  });
});
