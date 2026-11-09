# B 站夹具说明

这些 JSON 是 **T-0034 / T-0035 的离线测试夹具**，测试代码不访问真实平台。

## 实测与合成

2026-11-09 手工探测过真实接口（开发调试，不是测试），以下字段形状已实测：

- `nav`：未登录时 `code: -101`，但 `data.wbi_img` 仍返回 `img_url` / `sub_url`。
- `view`：`data.bvid`、`data.aid`、`data.cid`、`data.pages[].cid`。
- 搜索 `search/type`：`data.result[]` 的 `type` / `id` / `author` / `bvid` /
  `title`（含 `<em class="keyword">` 高亮）/ `pic`（`//` 开头）/ `duration`。
- `playurl`（`fnval=16`）：`dash.audio[]` 的 `baseUrl` / `backupUrl` /
  `bandwidth` / `mimeType`（实测均为 `audio/mp4`）/ `codecs`；URL 带
  `deadline` 秒级时间戳。`fnval=0` 时没有 `dash`，`durl[0].url` 是 `.mp4`。
- **CDN 防盗链（实测）**：不带任何头或只带 UA 均返回 **403**；带
  `Referer: https://www.bilibili.com` 返回 200；再加 `Origin` 与 `Range`
  返回 206。`HttpDownloader` 固定发送 `Referer` + `Origin`，**不要删**。

**合成部分**（未真实触发，按文档/形状构造，不要当作已验证）：

- `search_empty.json` 的 `result: null`；
- `search_denied.json` 的 `-412` 响应；
- 本目录所有 URL 已改写为 `https://example.invalid/...`；
- wbi key、deadline 数值、作者 / 标题 / BV 号均为合成或公共示例数据。

夹具不含 cookie、SESSDATA、账号信息或抓包原文。
