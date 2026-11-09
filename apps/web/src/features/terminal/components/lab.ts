/**
 * 终端实验台开关（T-0041）。
 *
 * 产品视图默认不含 M0 对比实验台（视图切换 / 主题色板 / 评分面板 /
 * 分屏 pane 头）；`?lab=1` 才显示。为兼容 M0 直链，`?variant=` 也视为实验台入口
 * （xterm 渲染器仍要能被测试和调试访问）。
 *
 * 纯函数：输入 query string，便于单测；组件不直接解析 URL。
 */
export function parseLab(search: string | null | undefined): boolean {
  if (!search) {
    return false;
  }

  const params = new URLSearchParams(search);

  return params.get("lab") === "1" || params.has("variant");
}
