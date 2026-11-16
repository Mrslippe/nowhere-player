# B 站直播 playinfo fixtures（B0 spike）

- 抓取日期：2026-11-17（Asia/Shanghai）
- 抓取方式：`getRoomPlayInfo` / `room/v1/Room/get_info`，带 `Referer: https://live.bilibili.com/`、`Origin: https://live.bilibili.com`
- 用途：`spikes/bili-live` 的离线回归夹具；**不含可播放的 signed URL**。

## 文件

| 文件 | 房间 | 来源 | 说明 |
|---|---|---|---|
| `live_playinfo.json` | 21622811（实际 room_id 21622811） | 真实响应 | `code=0`、`live_status=1`、`http_stream/flv`、`current_qn=250`、`audio_codecs.base=mp4a.40.2`（AAC-LC） |
| `offline_playinfo.json` | 510（实际 room_id 80397） | 真实响应 | `code=0`、`live_status=0`、`playurl_info=null` |
| `error_playinfo.json` | 99999999 | 真实响应 | `code=60004`、`message=房间不存在`、`data=null`；**非合成** |
| `room_info.json` | 21622811 | 真实响应 | 保留 `room_id` / `title` / `uid` / `live_status`，供标题与状态测试 |

所有 fixture 均为真实响应的脱敏快照，没有 synthetic 文件。直播状态会变化，抓取时的 `live_status` 不代表以后。

## 脱敏字段

`live_playinfo.json` / `offline_playinfo.json` 的 playurl 结构：

- `base_url`：替换为 `/redacted/stream.flv?`（原始值包含房间/主播/流 ID）。
- `url_info[].host`：替换为 `https://example.invalid`。
- `url_info[].extra`：替换为 `redacted=1`（原始值含 CDN 过期时间与签名参数）。
- `url_info`：只保留第一项，避免重复 CDN 条目。
- `session`：替换为 `redacted`。
- 其余结构、`code`、`live_status`、`codec_name`、`current_qn`、`audio_codecs`、`video_codecs`、`media_info`、`g_qn_desc` 保留。

`room_info.json`：

- `user_cover` / `keyframe` / `background` 及 `data` 下所有 `http(s)://` 字符串：替换为 `https://example.invalid/redacted`。
- `up_session`：清空（会话标识，不属于测试需要字段）。
- 保留 `uid` / `room_id` / `short_id` / `title` / `live_status` / 分区等。

`error_playinfo.json` 仅含错误码与错误文案，无需额外脱敏。

## 校验

已检查四个 JSON 文件：不含 CDN 主机、流路径、签名查询串或会话凭据。

## 备注

- 真实 API 的 `audio_codecs` 位于 `stream[].format[].codec[]` 内（不是顶层），值为 `mp4a.40.2`；FLV 容器内音频为 AAC。
- 房间 21622811 请求 `qn=80`，服务端返回 `current_qn=250`（720P）。
- fixture 只用于解析/选择逻辑测试，不能用于实际拉流。
