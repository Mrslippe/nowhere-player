/**
 * demo 页专用：读取浏览器计算值，用来展示「当前主题真实生效的 token」。
 * 业务代码不要引用本文件。
 */

/** 读取 :root 上某个 CSS 变量的计算值（十六进制/rgba 原样返回）。 */
export function tokenValue(token: string): string {
  return getComputedStyle(document.documentElement)
    .getPropertyValue(token)
    .trim();
}

/** 把 var(--x) 解析成浏览器归一化后的 rgb()/rgba() 字符串。 */
export function resolvedColor(token: string): string {
  const probe = document.createElement("span");
  probe.style.position = "absolute";
  probe.style.visibility = "hidden";
  probe.style.color = `var(${token})`;
  document.body.appendChild(probe);
  const value = getComputedStyle(probe).color;
  probe.remove();
  return value;
}

/** 解析字体栈实际生效的值（浏览器会保留回退列表）。 */
export function computedFontFamily(cssFontValue: string): string {
  const probe = document.createElement("span");
  probe.style.position = "absolute";
  probe.style.visibility = "hidden";
  probe.style.fontFamily = cssFontValue;
  document.body.appendChild(probe);
  const value = getComputedStyle(probe).fontFamily;
  probe.remove();
  return value;
}
