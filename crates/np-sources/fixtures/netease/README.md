# 网易云夹具说明

这些 JSON 是 **T-0036 的离线测试夹具**，测试代码不访问真实平台。

## 实测结论（2026-11-09，开发调试，不是测试）

- `https://music.163.com/api/search/get?s=<kw>&type=1&limit=20`
  **无需 cookie、无需任何 header 即可用**，返回 `code: 200`。
  老字段：`result.songs[].id/name/duration/artists[]/album{}`；
  新字段也可能出现：`dt/ar[]/al{}`。
  实测 `album` 里只有 `picId`、**没有 `picUrl`**（老接口），所以真实搜索结果
  通常拿不到封面；夹具里的 `picUrl` 是合成数据，用来覆盖映射与 https 校正。
- `https://music.163.com/api/song/enhance/player/url?ids=[<id>]&br=320000`
  **无需 cookie**。免费曲目 320k/192k/128k 都返回 `data[0].url`（实测是
  `http://m8xx.music.126.net/...`，`type: mp3`）；版权受限/会员曲目返回
  `url: null`、`data[0].code: -110`，HTTP 仍是 200。
- **`expi` 是剩余秒数（实测 1200），不是时间戳**；URL 路径里的时间戳也
  与“当前 + 1200s”一致。
- 网易云 CDN 直链**不需要 Referer/Origin**：无 header、仅 UA、加 Referer
  都是 200；`Range` 得到 206；`http://` 与 `https://` 都能取。
- 因为老接口可用，**没有引入 AES/RSA / weapi**。

## 合成部分（不要当作已验证）

- 所有 id、歌曲名、艺人、专辑、URL 都是合成值（URL 为 `example.invalid`）。
- 空搜索结果：网易云模糊搜索无法稳定触发真空，夹具为合成。
- -460（风控）、`expi` 缺失、恰好 30:00 边界：合成用例。
- 封面 `picUrl`：真实老接口不返回，夹具为合成。
