import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Block } from "./terminal-model";
import {
  THEME_APPLY_COMMAND,
  THEME_CHANGED_EVENT,
  THEME_EXPORT_COMMAND,
  THEME_INITIAL_COMMAND,
  THEME_LIST_COMMAND,
  THEME_STORAGE_KEY,
  THEME_TOKEN_NAMES,
  SENTINEL_DELAY_MS,
  MIN_CONTRAST_RATIO,
  applyStructure,
  applyThemeById,
  checkSentinel,
  clearSelectedThemeId,
  contrastRatio,
  cssVarName,
  exportBuiltinTheme,
  installThemeLoader,
  isThemeApplyReply,
  isThemeColor,
  listThemes,
  resolveStructure,
  loadSelectedThemeId,
  resetThemeLoader,
  saveSelectedThemeId,
  themeBrokenBlock,
  type ThemeApplyReply,
  type ThemeEnvironment,
  type ThemeTokens,
  type ThemeTransport,
} from "./theme-loader";

/** 一组全合法的 token：高对比（白字黑底），不触发 hint。 */
const TOKENS: ThemeTokens = {
  bg: "#000000",
  bgElev: "#101010",
  bgHover: "#202020",
  fg: "#ffffff",
  fgMuted: "#cccccc",
  fgFaint: "#888888",
  accent: "#7c9ef5",
  accentSoft: "rgba(124, 158, 245, 0.14)",
  accentFg: "#000000",
  danger: "#f2707a",
  warn: "#e8b45c",
  success: "#7fc79a",
  line: "rgba(255, 255, 255, 0.07)",
  lineStrong: "rgba(255, 255, 255, 0.14)",
};

interface FakeEnvironment {
  env: ThemeEnvironment;
  style: Map<string, string>;
  dataset: Record<string, string | undefined>;
  storage: Map<string, string>;
  blocks: Block[];
  scheduled: Array<{ callback: () => void; delayMs: number }>;
}

interface FakeOptions {
  /** `null` 表示页面里找不到哨兵元素。 */
  rect?: { width: number; height: number } | null;
  hidden?: boolean;
}

function fakeEnvironment(options: FakeOptions = {}): FakeEnvironment {
  const style = new Map<string, string>();
  const dataset: Record<string, string | undefined> = {};
  const storage = new Map<string, string>();
  const blocks: Block[] = [];
  const scheduled: Array<{ callback: () => void; delayMs: number }> = [];

  const rect = options.rect === undefined ? { width: 320, height: 120 } : options.rect;
  const sentinel =
    rect === null ? null : { getBoundingClientRect: () => ({ ...rect }) };

  const env: ThemeEnvironment = {
    root: {
      style: {
        setProperty: (name, value) => {
          style.set(name, value);
        },
        removeProperty: (name) => {
          style.delete(name);
        },
      },
      dataset,
    },
    querySelector: () => sentinel,
    isHidden: () => options.hidden ?? false,
    schedule: (callback, delayMs) => {
      scheduled.push({ callback, delayMs });
    },
    emitBlock: (block) => {
      blocks.push(block);
    },
    storage: {
      get: (key) => storage.get(key) ?? null,
      set: (key, value) => {
        storage.set(key, value);
      },
      remove: (key) => {
        storage.delete(key);
      },
    },
    invoke: async () => {
      throw new Error("测试未注入 IPC");
    },
  };

  return { env, style, dataset, storage, blocks, scheduled };
}

function makeReply(overrides: Partial<ThemeApplyReply> = {}): ThemeApplyReply {
  return {
    id: "my-theme",
    name: "我的主题",
    skin: "plain",
    source: "user",
    tokens: { ...TOKENS },
    warnings: [],
    ...overrides,
  };
}

function transportFor(
  reply: ThemeApplyReply,
  applied: string[] = [],
): ThemeTransport {
  return {
    list: async () => [],
    apply: async (id) => {
      applied.push(id);
      return reply;
    },
    exportBuiltin: async (id) => ({ path: `/themes/${id}.json`, created: true }),
  };
}

function firstOutput(block: Block | undefined) {
  return block?.outputs[0];
}

beforeEach(() => {
  resetThemeLoader();
});

afterEach(() => {
  vi.unstubAllGlobals();
  resetThemeLoader();
});

describe("主题加载器", () => {
  it("应用主题：tokens 写成 inline CSS 变量，data-skin / data-theme 同步", async () => {
    const fake = fakeEnvironment();

    const reply = await applyThemeById("my-theme", {
      env: fake.env,
      transport: transportFor(makeReply()),
    });

    expect(reply?.id).toBe("my-theme");
    expect(fake.style.get("--bg")).toBe("#000000");
    expect(fake.style.get("--fg-muted")).toBe("#cccccc");
    expect(fake.style.get("--line-strong")).toBe("rgba(255, 255, 255, 0.14)");
    expect(fake.style.get("color-scheme")).toBe("dark");
    expect(fake.dataset.skin).toBe("plain");
    expect(fake.dataset.theme).toBe("my-theme");
    // 14 个 token 全写进去。
    expect(THEME_TOKEN_NAMES.every((token) => fake.style.has(cssVarName(token)))).toBe(true);
    // T-0056：缺省 structure 按 plain 预设补全，8 个属性齐全。
    expect(fake.dataset).toMatchObject({
      structureRadius: "soft",
      structureBorder: "hairline",
      structureDensity: "normal",
      structureProgress: "line",
      structureSelection: "bar",
      structureLabels: "sentence",
      structureStripe: "off",
      structureFont: "mixed",
    });
  });

  it("structure：合法值覆盖、非法轴逐项回落 skin 预设", () => {
    const fake = fakeEnvironment();
    applyStructure(fake.env.root, "phosphor", {
      radius: "soft",
      border: "box",
      density: "bogus",
      selection: "reverse",
    });

    expect(fake.dataset).toMatchObject({
      structureRadius: "soft",
      structureBorder: "box",
      structureDensity: "compact",
      structureProgress: "line",
      structureSelection: "reverse",
      structureLabels: "sentence",
      structureStripe: "off",
      structureFont: "mono",
    });
  });

  it("structure：非法输入整体回落，不抛错", () => {
    const fake = fakeEnvironment();
    const resolved = applyStructure(fake.env.root, "plain", "not-an-object");

    expect(resolved).toMatchObject({
      radius: "soft",
      border: "hairline",
      density: "normal",
      font: "mixed",
    });
    expect(
      resolveStructure("plain", { progress: "bracket", font: "mono" }).progress,
    ).toBe("bracket");
    // 默认 font=mixed 时，字符块进度按规则回退 line。
    expect(resolveStructure("plain", { progress: "bracket" }).progress).toBe(
      "line",
    );
  });

  it("structure：labels=tracked，reverse+stripe 强制 off，blocks+mixed 回退 line", () => {
    expect(resolveStructure("plain", { labels: "tracked" }).labels).toBe("tracked");
    // 旧值 upper 已作废，按非法回落。
    expect(resolveStructure("plain", { labels: "upper" }).labels).toBe("sentence");
    expect(
      resolveStructure("plain", { selection: "reverse", stripe: "on" }).stripe,
    ).toBe("off");
    expect(
      resolveStructure("plain", { progress: "blocks", font: "mixed" }).progress,
    ).toBe("line");
    expect(
      resolveStructure("plain", { progress: "blocks", font: "mono" }).progress,
    ).toBe("blocks");
  });

  it("切换主题清理上一份残留，非法颜色跳过而不是写脏值", async () => {
    const fake = fakeEnvironment();
    fake.dataset.skin = "phosphor";

    await applyThemeById("first", {
      env: fake.env,
      transport: transportFor(makeReply({ id: "first", skin: "phosphor" })),
    });
    expect(fake.style.has("--line")).toBe(true);

    const tokens: ThemeTokens = { ...TOKENS, line: "rebeccapurple" };
    const applied = await applyThemeById("second", {
      env: fake.env,
      transport: transportFor(makeReply({ id: "second", tokens })),
    });

    expect(applied?.id).toBe("second");
    // 非法值被跳过：上一份的 --line 被清掉，且不会用脏值覆盖。
    expect(fake.style.has("--line")).toBe(false);
    expect(fake.style.get("--bg")).toBe("#000000");
    expect(fake.dataset.theme).toBe("second");
  });

  it("fg/bg 对比度低于 4.5:1 时推一条 hint，但不阻止应用", async () => {
    const fake = fakeEnvironment();
    const tokens: ThemeTokens = { ...TOKENS, bg: "#101010", fg: "#101010" };

    const reply = await applyThemeById("low-contrast", {
      env: fake.env,
      transport: transportFor(makeReply({ id: "low-contrast", tokens })),
    });

    expect(reply).not.toBeNull();
    expect(fake.dataset.theme).toBe("low-contrast");
    const output = firstOutput(fake.blocks[0]);
    expect(output?.type).toBe("hint");
    if (output?.type === "hint") {
      expect(output.text).toContain("1.00:1");
      expect(output.text).toContain("4.5:1");
      expect(output.suggestion).toContain("tokens.fg");
    }
  });

  it("对比度足够时不推 hint", async () => {
    const fake = fakeEnvironment();

    await applyThemeById("ok", {
      env: fake.env,
      transport: transportFor(makeReply()),
    });

    expect(contrastRatio("#ffffff", "#000000")).toBeCloseTo(21, 5);
    expect(fake.blocks).toHaveLength(0);
  });

  it("安全模式：哨兵尺寸为 0 时回退 graphite、清掉选择并推 E_THEME_BROKEN", async () => {
    const fake = fakeEnvironment({ rect: { width: 0, height: 0 } });

    await applyThemeById("broken-theme", {
      env: fake.env,
      transport: transportFor(makeReply({ id: "broken-theme" })),
    });

    expect(fake.dataset.theme).toBe("broken-theme");
    expect(fake.scheduled).toHaveLength(1);
    expect(fake.scheduled[0].delayMs).toBe(SENTINEL_DELAY_MS);

    fake.scheduled[0].callback();

    expect(fake.dataset.theme).toBe("graphite");
    expect(fake.dataset.skin).toBe("plain");
    // inline 变量已清空，只剩 color-scheme。
    expect([...fake.style.keys()]).toEqual(["color-scheme"]);
    expect(fake.storage.has(THEME_STORAGE_KEY)).toBe(false);

    const output = firstOutput(fake.blocks[0]);
    expect(output?.type).toBe("error");
    if (output?.type === "error") {
      expect(output.code).toBe("E_THEME_BROKEN");
      expect(output.message).toContain("broken-theme");
    }
    // 只尝试一次：不会再排入第二次检查。
    expect(fake.scheduled).toHaveLength(1);
  });

  it("安全模式通过：尺寸正常时保持主题，不回退", async () => {
    const fake = fakeEnvironment({ rect: { width: 320, height: 120 } });

    await applyThemeById("fine", {
      env: fake.env,
      transport: transportFor(makeReply({ id: "fine" })),
    });
    fake.scheduled[0].callback();

    expect(fake.dataset.theme).toBe("fine");
    expect(fake.style.get("--bg")).toBe("#000000");
    expect(fake.blocks).toHaveLength(0);
  });

  it("窗口隐藏或找不到哨兵时不判崩坏（避免隐藏窗误回退）", async () => {
    const hidden = fakeEnvironment({ hidden: true });
    await applyThemeById("hidden-window", {
      env: hidden.env,
      transport: transportFor(makeReply({ id: "hidden-window" })),
    });
    hidden.scheduled[0].callback();

    const missing = fakeEnvironment({ rect: null });
    await applyThemeById("no-sentinel", {
      env: missing.env,
      transport: transportFor(makeReply({ id: "no-sentinel" })),
    });
    missing.scheduled[0].callback();

    expect(hidden.dataset.theme).toBe("hidden-window");
    expect(missing.dataset.theme).toBe("no-sentinel");
    expect(hidden.blocks).toHaveLength(0);
    expect(missing.blocks).toHaveLength(0);
    expect(checkSentinel("x", hidden.env)).toBe("skipped");
  });

  it("应用失败（IPC / 文件非法）推错误块并保留 hint，不写任何变量", async () => {
    const fake = fakeEnvironment();
    const transport: ThemeTransport = {
      list: async () => [],
      apply: async () => {
        throw {
          code: "E_THEME_NOT_FOUND",
          message: "主题不存在：nope",
          hint: "用 theme.list 查看可用主题",
        };
      },
      exportBuiltin: async () => ({ path: "", created: false }),
    };

    const reply = await applyThemeById("nope", { env: fake.env, transport });

    expect(reply).toBeNull();
    expect(fake.style.size).toBe(0);
    const output = firstOutput(fake.blocks[0]);
    expect(output?.type).toBe("error");
    if (output?.type === "error") {
      expect(output.code).toBe("E_THEME_NOT_FOUND");
      expect(output.hint).toBe("用 theme.list 查看可用主题");
    }
  });

  it("非法回执被拒（不把脏值写进 CSS）", async () => {
    const fake = fakeEnvironment();
    const transport: ThemeTransport = {
      list: async () => [],
      apply: async () => ({ id: "x" }) as unknown as ThemeApplyReply,
      exportBuiltin: async () => ({ path: "", created: false }),
    };

    const reply = await applyThemeById("x", { env: fake.env, transport });

    expect(reply).toBeNull();
    expect(fake.style.size).toBe(0);
    const output = firstOutput(fake.blocks[0]);
    expect(output?.type === "error" ? output.code : null).toBe("E_THEME_JSON");
  });

  it("启动恢复：用 np.theme.id 恢复用户主题", async () => {
    const fake = fakeEnvironment();
    fake.storage.set(THEME_STORAGE_KEY, "mine");
    const applied: string[] = [];

    const reply = await installThemeLoader({
      env: fake.env,
      transport: transportFor(makeReply({ id: "mine" }), applied),
      force: true,
    });

    expect(applied).toEqual(["mine"]);
    expect(reply?.id).toBe("mine");
    expect(fake.dataset.theme).toBe("mine");
  });

  it("启动恢复：URL 指定内置主题时交给 CSS，不写 inline；重复安装只跑一次", async () => {
    const fake = fakeEnvironment();
    const applied: string[] = [];
    const options = {
      env: fake.env,
      transport: transportFor(makeReply(), applied),
      force: true,
    };

    vi.stubGlobal("window", {
      location: { search: "?theme=amber" },
    });

    expect(await installThemeLoader(options)).toBeNull();
    expect(await installThemeLoader(options)).toBeNull();
    expect(applied).toEqual([]);
    expect(fake.dataset.theme).toBeUndefined();
  });

  it("选择持久化只记录用户主题，内置主题不落 np.theme.id", async () => {
    const fake = fakeEnvironment();
    saveSelectedThemeId("mine", fake.env.storage);
    expect(fake.storage.get(THEME_STORAGE_KEY)).toBe("mine");
    expect(loadSelectedThemeId(fake.env.storage)).toBe("mine");

    saveSelectedThemeId("amber", fake.env.storage);
    expect(loadSelectedThemeId(fake.env.storage)).toBe("mine");

    clearSelectedThemeId(fake.env.storage);
    expect(fake.storage.has(THEME_STORAGE_KEY)).toBe(false);
    expect(loadSelectedThemeId(fake.env.storage)).toBeNull();
  });

  it("list / exportBuiltin 通过注入的传输层走通", async () => {
    const transport = transportFor(makeReply());
    expect(await listThemes({ transport })).toEqual([]);
    expect(await exportBuiltinTheme("amber", { transport })).toEqual({
      path: "/themes/amber.json",
      created: true,
    });
  });


  it("启动恢复：settings 的 theme.id 优先于 URL/localStorage", async () => {
    const fake = fakeEnvironment();
    const applied: string[] = [];
    const transport = {
      ...transportFor(makeReply({ id: "ice", source: "builtin" }), applied),
      initial: async () => "ice",
    };

    const reply = await installThemeLoader({
      env: fake.env,
      transport,
      force: true,
      preferSettings: true,
    });

    expect(applied).toEqual(["ice"]);
    expect(reply?.id).toBe("ice");
    expect(fake.dataset.theme).toBe("ice");
  });

  it("监听 theme:changed 并在本窗口应用", async () => {
    const fake = fakeEnvironment();
    let handler: ((id: string) => void) | undefined;
    fake.env.onThemeChanged = (next) => {
      handler = next;
      return () => {};
    };
    const applied: string[] = [];
    const transport = transportFor(makeReply({ id: "amber", source: "builtin" }), applied);

    await installThemeLoader({
      env: fake.env,
      transport,
      force: true,
      skipInitial: true,
    });
    expect(handler).toBeDefined();
    handler?.("amber");

    await vi.waitFor(() => {
      expect(applied).toEqual(["amber"]);
    });
    expect(fake.dataset.theme).toBe("amber");
  });

  it("命令名与阈值是冻结契约", () => {
    expect(THEME_LIST_COMMAND).toBe("theme.list");
    expect(THEME_APPLY_COMMAND).toBe("theme.apply");
    expect(THEME_EXPORT_COMMAND).toBe("theme.exportBuiltin");
    expect(THEME_INITIAL_COMMAND).toBe("theme.initial");
    expect(THEME_CHANGED_EVENT).toBe("theme:changed");
    expect(THEME_STORAGE_KEY).toBe("np.theme.id");
    expect(SENTINEL_DELAY_MS).toBe(300);
    expect(MIN_CONTRAST_RATIO).toBe(4.5);
  });

  it("颜色校验与回执校验是严格的", () => {
    for (const value of [
      "#fff",
      "#A1B2C3",
      "#a1b2c3d4",
      "rgba(0,0,0,.5)",
      "rgba(255, 255, 255, 0.07)",
    ]) {
      expect(isThemeColor(value)).toBe(true);
    }

    for (const value of ["", "#ff", "rgb(0,0,0)", "rgba(0,0,0)", "rgba(300,0,0,1)", "red"]) {
      expect(isThemeColor(value)).toBe(false);
    }

    expect(isThemeApplyReply(makeReply())).toBe(true);
    expect(isThemeApplyReply({})).toBe(false);
    expect(isThemeApplyReply({ ...makeReply(), skin: "device" })).toBe(false);
  });

  it("E_THEME_BROKEN 块的 id 固定，便于按 id 覆盖去重", () => {
    const block = themeBrokenBlock("x", 1);
    expect(block.id).toBe("blk_theme_broken");
    expect(block.outputs[0]?.type).toBe("error");
  });
});
