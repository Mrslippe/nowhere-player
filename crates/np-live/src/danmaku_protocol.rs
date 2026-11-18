//! B 站直播弹幕协议数据层（T-0100a）。
//!
//! 只做字节层：16 字节包头 encode/decode、多包拆分、心跳与认证 body、
//! `DANMU_MSG` JSON 解析。**不建立连接、不做压缩、不引入依赖**；WS 传输与
//! zlib/brotli 解压留给后续 T-0100a2。
//!
//! 弹幕包全部使用大端序：
//!
//! ```text
//! 0..4   total_len   u32  包头 + body 的总字节数
//! 4..6   header_len  u16  固定 16
//! 6..8   protover    u16  1 明文 / 2、3 压缩（本层不处理）
//! 8..12  operation   u32  2 心跳、3 心跳回复、5 通知、7 认证、8 认证回复
//! 12..16 sequence    u32
//! ```
//!
//! 认证 body 与心跳 body 由调用方通过 [`encode_packet`] 装包；`op=5` 的 body
//! 再用 [`parse_danmu_msg`] 解析。所有外部字节都先检查长度，坏包返回
//! [`DanmakuProtocolError`]，不会 panic。

use serde::Serialize;
use serde_json::Value;

/// 包头的固定长度（字节）。
pub const HEADER_LEN: usize = 16;

/// 客户端心跳。
pub const OP_HEARTBEAT: u32 = 2;
/// 服务端心跳回复（body 为 4 字节人气值）。
pub const OP_HEARTBEAT_REPLY: u32 = 3;
/// 通知，body 是 JSON。
pub const OP_NOTIFICATION: u32 = 5;
/// 客户端认证。
pub const OP_AUTH: u32 = 7;
/// 服务端认证回复。
pub const OP_AUTH_REPLY: u32 = 8;

/// 本模块写出的包固定使用明文协议版本 1。
const PROTOVER_PLAIN: u16 = 1;
/// 包头里的 sequence，B 站客户端通常固定为 1。
const SEQUENCE: u32 = 1;

/// 一个已拆出的弹幕包。
///
/// `body` 原样保留（可能是 JSON、认证响应或压缩数据）；压缩包的 `protover`
/// 会被丢弃，解压需要后续协议扩展。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanmakuPacket {
    /// 操作码，见 [`OP_HEARTBEAT`] / [`OP_AUTH`] 等常量。
    pub operation: u32,
    /// 包头之后的原始 body。
    pub body: Vec<u8>,
}

/// 协议数据层错误。
///
/// 只描述“哪一步、差多少字节、缺哪个字段”，不携带连接信息或凭证。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DanmakuProtocolError {
    /// 缓冲区不足一个完整包头。
    #[error("弹幕包数据不足：需要 {expected} 字节，实际 {actual} 字节")]
    TooShort { expected: usize, actual: usize },

    /// `total_len` 小于固定包头长度。
    #[error("弹幕包总长度 {total_len} 小于固定包头 16 字节")]
    InvalidTotalLength { total_len: u32 },

    /// `header_len` 小于 16，或大于 `total_len`。
    #[error("弹幕包头长度 {header_len} 非法（总长度 {total_len}）")]
    InvalidHeaderLength { header_len: u16, total_len: u32 },

    /// `total_len` 超过当前缓冲区，包被截断。
    #[error("弹幕包声明 {declared} 字节，但缓冲区只剩 {available} 字节")]
    Truncated { declared: u32, available: usize },

    /// body 不是合法 JSON。
    #[error("弹幕 JSON 无法解析：{0}")]
    InvalidJson(String),

    /// JSON 缺少必要字段；字段路径使用 `info[0][3]` 形式。
    #[error("弹幕 JSON 缺少字段 {0}")]
    MissingField(&'static str),

    /// JSON 字段类型或取值范围无效。
    #[error("弹幕 JSON 字段 {0} 类型或范围无效")]
    InvalidField(&'static str),
}

/// 按 B 站格式编码一个包。
///
/// 包头为小端主机序写出的大端字节；`header_len=16`、`protover=1`、
/// `sequence=1`。body 超过 `u32` 可表示范围时总长度会饱和（实际弹幕包远小于
/// 该上限，传输层会先拒绝超大帧）。
pub fn encode_packet(operation: u32, body: &[u8]) -> Vec<u8> {
    let total_len = u32::try_from(body.len().saturating_add(HEADER_LEN)).unwrap_or(u32::MAX);
    let mut bytes = Vec::with_capacity(HEADER_LEN + body.len());
    bytes.extend_from_slice(&total_len.to_be_bytes());
    bytes.extend_from_slice(&(HEADER_LEN as u16).to_be_bytes());
    bytes.extend_from_slice(&PROTOVER_PLAIN.to_be_bytes());
    bytes.extend_from_slice(&operation.to_be_bytes());
    bytes.extend_from_slice(&SEQUENCE.to_be_bytes());
    bytes.extend_from_slice(body);
    bytes
}

/// 把一段连续字节拆成多个包。
///
/// 缓冲区为空时返回空列表；末尾不足 16 字节、声明长度超过缓冲区、包头长度
/// 非法都返回错误，不会静默丢弃残包。
pub fn decode_packets(bytes: &[u8]) -> Result<Vec<DanmakuPacket>, DanmakuProtocolError> {
    let mut packets = Vec::new();
    let mut offset = 0usize;

    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        if remaining.len() < HEADER_LEN {
            return Err(DanmakuProtocolError::TooShort {
                expected: HEADER_LEN,
                actual: remaining.len(),
            });
        }
        let header = &remaining[..HEADER_LEN];
        let total_len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let header_len = u16::from_be_bytes([header[4], header[5]]);
        let operation = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
        let total_len_usize = total_len as usize;

        if total_len_usize < HEADER_LEN {
            return Err(DanmakuProtocolError::InvalidTotalLength { total_len });
        }
        if usize::from(header_len) < HEADER_LEN || usize::from(header_len) > total_len_usize {
            return Err(DanmakuProtocolError::InvalidHeaderLength {
                header_len,
                total_len,
            });
        }
        if total_len_usize > remaining.len() {
            return Err(DanmakuProtocolError::Truncated {
                declared: total_len,
                available: remaining.len(),
            });
        }

        // 上面的检查保证 16 <= header_len <= total_len <= remaining.len()。
        let body = remaining[usize::from(header_len)..total_len_usize].to_vec();
        packets.push(DanmakuPacket { operation, body });
        offset += total_len_usize;
    }

    Ok(packets)
}

/// 认证 body（`op=7`），字段顺序与 B 站 Web 客户端一致。
///
/// `uid` 未登录传 0；`protover` 建议传 3，服务端才允许下发压缩通知；
/// `key` 是弹幕服务器 token，本层不读取、不打印。
pub fn auth_body(room_id: u64, uid: u64, protover: u16, key: &str) -> Vec<u8> {
    #[derive(Serialize)]
    struct AuthBody<'a> {
        roomid: u64,
        uid: u64,
        protover: u16,
        platform: &'static str,
        #[serde(rename = "type")]
        kind: u8,
        key: &'a str,
    }

    let body = AuthBody {
        roomid: room_id,
        uid,
        protover,
        platform: "web",
        kind: 2,
        key,
    };
    // 结构体只含整数与字符串，序列化不会失败；失败时返回空 body，由服务端
    // 认证回复暴露问题，而不是 panic。
    serde_json::to_vec(&body).unwrap_or_default()
}

/// 心跳 body（`op=2`）。
///
/// B 站 Web 客户端发送的固定文本就是 `[object Object]`；服务端只校验包结构，
/// 不解析内容。
pub fn heartbeat_body() -> &'static [u8] {
    b"[object Object]"
}

/// 一条 `DANMU_MSG` 弹幕。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanmakuMessage {
    /// 发送者昵称。
    pub user: String,
    /// 弹幕文本。
    pub text: String,
    /// 十进制 RGB，例如 `16777215` 为白色。
    pub color: u32,
    /// 发送时间，Unix 毫秒。
    pub timestamp: u64,
}

/// 解析一条 `op=5` 通知的 body。
///
/// 只认 `cmd == "DANMU_MSG"` 或 `DANMU_MSG:` 前缀变体（如
/// `DANMU_MSG:4:0:2:2:2:0`）；其它 `cmd`（礼物、进场等）返回 `Ok(None)`。
/// `DANMU_MSG` 缺字段或类型不符返回 [`DanmakuProtocolError`]。
pub fn parse_danmu_msg(json: &[u8]) -> Result<Option<DanmakuMessage>, DanmakuProtocolError> {
    let value: Value = serde_json::from_slice(json)
        .map_err(|error| DanmakuProtocolError::InvalidJson(error.to_string()))?;

    let cmd = value
        .get("cmd")
        .ok_or(DanmakuProtocolError::MissingField("cmd"))?;
    let cmd = cmd
        .as_str()
        .ok_or(DanmakuProtocolError::InvalidField("cmd"))?;
    let is_danmu = cmd == "DANMU_MSG" || cmd.starts_with("DANMU_MSG:");
    if !is_danmu {
        return Ok(None);
    }

    let info = value
        .get("info")
        .ok_or(DanmakuProtocolError::MissingField("info"))?;
    let info = info
        .as_array()
        .ok_or(DanmakuProtocolError::InvalidField("info"))?;
    // info[0] = [mode, fontsize, ..., color, timestamp_ms, ...]
    let meta = info
        .first()
        .ok_or(DanmakuProtocolError::MissingField("info[0]"))?;
    let meta = meta
        .as_array()
        .ok_or(DanmakuProtocolError::InvalidField("info[0]"))?;
    // info[1] = 弹幕文本；info[2][1] = 发送者昵称。
    let text = info
        .get(1)
        .ok_or(DanmakuProtocolError::MissingField("info[1]"))?;
    let text = text
        .as_str()
        .ok_or(DanmakuProtocolError::InvalidField("info[1]"))?;
    let user = info
        .get(2)
        .ok_or(DanmakuProtocolError::MissingField("info[2]"))?;
    let user = user
        .as_array()
        .ok_or(DanmakuProtocolError::InvalidField("info[2]"))?;
    let user = user
        .get(1)
        .ok_or(DanmakuProtocolError::MissingField("info[2][1]"))?;
    let user = user
        .as_str()
        .ok_or(DanmakuProtocolError::InvalidField("info[2][1]"))?;

    let color = meta
        .get(3)
        .ok_or(DanmakuProtocolError::MissingField("info[0][3]"))?;
    let color = color
        .as_u64()
        .ok_or(DanmakuProtocolError::InvalidField("info[0][3]"))?;
    let color =
        u32::try_from(color).map_err(|_| DanmakuProtocolError::InvalidField("info[0][3]"))?;

    let timestamp = meta
        .get(4)
        .ok_or(DanmakuProtocolError::MissingField("info[0][4]"))?;
    let timestamp = timestamp
        .as_u64()
        .ok_or(DanmakuProtocolError::InvalidField("info[0][4]"))?;

    Ok(Some(DanmakuMessage {
        user: user.to_owned(),
        text: text.to_owned(),
        color,
        timestamp,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实结构的 `DANMU_MSG`：meta 前五项依次包含颜色与毫秒时间戳。
    const DANMU_JSON: &str = r#"{
        "cmd": "DANMU_MSG",
        "info": [
            [0, 1, 25, 16777215, 1726799123456, 1726799123, 0, "hash", 0, 0, 0, "", 0, "{}", "{}", 0],
            "晚上好",
            [123456, "用户A", 0, 0, 0, 10000, 1, ""]
        ]
    }"#;

    fn packet_header(total_len: u32, header_len: u16, operation: u32) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_LEN);
        bytes.extend_from_slice(&total_len.to_be_bytes());
        bytes.extend_from_slice(&header_len.to_be_bytes());
        bytes.extend_from_slice(&PROTOVER_PLAIN.to_be_bytes());
        bytes.extend_from_slice(&operation.to_be_bytes());
        bytes.extend_from_slice(&SEQUENCE.to_be_bytes());
        bytes
    }

    fn danmu_value() -> Value {
        serde_json::from_str(DANMU_JSON).expect("测试夹具必须是合法 JSON")
    }

    #[test]
    fn empty_buffer_yields_no_packets() {
        assert_eq!(decode_packets(&[]), Ok(Vec::new()));
    }

    #[test]
    fn buffer_shorter_than_header_errors() {
        assert_eq!(
            decode_packets(&[0u8; 15]),
            Err(DanmakuProtocolError::TooShort {
                expected: HEADER_LEN,
                actual: 15,
            })
        );
        assert_eq!(
            decode_packets(&[0u8; 1]),
            Err(DanmakuProtocolError::TooShort {
                expected: HEADER_LEN,
                actual: 1,
            })
        );
    }

    #[test]
    fn total_length_below_header_errors() {
        let bytes = packet_header(8, 16, OP_NOTIFICATION);
        assert_eq!(
            decode_packets(&bytes),
            Err(DanmakuProtocolError::InvalidTotalLength { total_len: 8 })
        );
    }

    #[test]
    fn header_length_below_fixed_errors() {
        let bytes = packet_header(16, 12, OP_NOTIFICATION);
        assert_eq!(
            decode_packets(&bytes),
            Err(DanmakuProtocolError::InvalidHeaderLength {
                header_len: 12,
                total_len: 16,
            })
        );
    }

    #[test]
    fn header_length_beyond_total_errors() {
        let bytes = packet_header(16, 20, OP_NOTIFICATION);
        assert_eq!(
            decode_packets(&bytes),
            Err(DanmakuProtocolError::InvalidHeaderLength {
                header_len: 20,
                total_len: 16,
            })
        );
    }

    #[test]
    fn declared_length_beyond_buffer_errors() {
        let bytes = packet_header(20, 16, OP_NOTIFICATION);
        assert_eq!(
            decode_packets(&bytes),
            Err(DanmakuProtocolError::Truncated {
                declared: 20,
                available: 16,
            })
        );
    }

    #[test]
    fn encode_writes_expected_header() {
        let encoded = encode_packet(OP_AUTH, b"{}");
        assert_eq!(encoded.len(), 18);
        assert_eq!(encoded[0..4], 18u32.to_be_bytes());
        assert_eq!(encoded[4..6], 16u16.to_be_bytes());
        assert_eq!(encoded[6..8], 1u16.to_be_bytes());
        assert_eq!(encoded[8..12], OP_AUTH.to_be_bytes());
        assert_eq!(encoded[12..16], 1u32.to_be_bytes());
        assert_eq!(encoded[16..], *b"{}");
    }

    #[test]
    fn empty_body_round_trips() {
        let encoded = encode_packet(OP_HEARTBEAT, b"");
        assert_eq!(
            decode_packets(&encoded),
            Ok(vec![DanmakuPacket {
                operation: OP_HEARTBEAT,
                body: Vec::new(),
            }])
        );
    }

    #[test]
    fn multiple_packets_are_split() {
        let mut stream = encode_packet(OP_NOTIFICATION, DANMU_JSON.as_bytes());
        stream.extend_from_slice(&encode_packet(OP_HEARTBEAT_REPLY, &[0, 0, 0, 42]));
        stream.extend_from_slice(&encode_packet(OP_AUTH_REPLY, b"{\"code\":0}"));

        let packets = decode_packets(&stream).expect("三个完整包应全部解出");
        assert_eq!(packets.len(), 3);
        assert_eq!(packets[0].operation, OP_NOTIFICATION);
        assert_eq!(packets[0].body, DANMU_JSON.as_bytes());
        assert_eq!(packets[1].operation, OP_HEARTBEAT_REPLY);
        assert_eq!(packets[1].body, [0, 0, 0, 42]);
        assert_eq!(packets[2].operation, OP_AUTH_REPLY);
        assert_eq!(packets[2].body, b"{\"code\":0}");
    }

    #[test]
    fn header_longer_than_sixteen_is_honored() {
        let body = b"payload";
        let mut bytes = packet_header(20 + body.len() as u32, 20, OP_NOTIFICATION);
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(body);
        assert_eq!(
            decode_packets(&bytes),
            Ok(vec![DanmakuPacket {
                operation: OP_NOTIFICATION,
                body: body.to_vec(),
            }])
        );
    }

    #[test]
    fn trailing_short_fragment_errors() {
        let mut stream = encode_packet(OP_HEARTBEAT, b"");
        stream.extend_from_slice(&[0u8; 5]);
        assert!(matches!(
            decode_packets(&stream),
            Err(DanmakuProtocolError::TooShort {
                expected: HEADER_LEN,
                actual: 5,
            })
        ));
    }

    #[test]
    fn every_truncation_of_a_valid_packet_errors() {
        let packet = encode_packet(OP_NOTIFICATION, DANMU_JSON.as_bytes());
        for cut in 1..packet.len() {
            assert!(
                decode_packets(&packet[..cut]).is_err(),
                "截断到 {cut} 字节应返回错误"
            );
        }
    }

    #[test]
    fn arbitrary_buffers_do_not_panic() {
        for len in 0..64 {
            let bytes = vec![0xFFu8; len];
            let _ = decode_packets(&bytes);
            let bytes = vec![0x00u8; len];
            let _ = decode_packets(&bytes);
        }
    }

    #[test]
    fn heartbeat_body_is_object_object() {
        assert_eq!(heartbeat_body(), b"[object Object]");
    }

    #[test]
    fn auth_body_contains_expected_fields() {
        let body = auth_body(12345, 0, 3, "token-abc");
        let value: Value = serde_json::from_slice(&body).expect("认证 body 必须是合法 JSON");
        assert_eq!(value["roomid"], serde_json::json!(12345));
        assert_eq!(value["uid"], serde_json::json!(0));
        assert_eq!(value["protover"], serde_json::json!(3));
        assert_eq!(value["platform"], serde_json::json!("web"));
        assert_eq!(value["type"], serde_json::json!(2));
        assert_eq!(value["key"], serde_json::json!("token-abc"));
    }

    #[test]
    fn auth_body_escapes_key() {
        let key = "a\"b\\c\n令牌";
        let body = auth_body(1, 2, 3, key);
        let value: Value = serde_json::from_slice(&body).expect("认证 body 必须是合法 JSON");
        assert_eq!(value["key"], serde_json::json!(key));
    }

    #[test]
    fn auth_packet_round_trips() {
        let body = auth_body(12345, 0, 3, "token-abc");
        let encoded = encode_packet(OP_AUTH, &body);
        let packets = decode_packets(&encoded).expect("认证包应能解出");
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].operation, OP_AUTH);
        assert_eq!(packets[0].body, body);
    }

    #[test]
    fn parse_danmu_msg_reads_user_text_color_timestamp() {
        let message = parse_danmu_msg(DANMU_JSON.as_bytes())
            .expect("合法弹幕不应报错")
            .expect("DANMU_MSG 应返回 Some");
        assert_eq!(message.user, "用户A");
        assert_eq!(message.text, "晚上好");
        assert_eq!(message.color, 16_777_215);
        assert_eq!(message.timestamp, 1_726_799_123_456);
    }

    #[test]
    fn parse_danmu_msg_accepts_cmd_suffix() {
        let json = DANMU_JSON.replace("\"DANMU_MSG\"", "\"DANMU_MSG:4:0:2:2:2:0\"");
        let message = parse_danmu_msg(json.as_bytes())
            .expect("变体 cmd 不应报错")
            .expect("DANMU_MSG 变体应返回 Some");
        assert_eq!(message.text, "晚上好");
    }

    #[test]
    fn non_danmaku_cmd_returns_none() {
        let json = serde_json::json!({"cmd": "SEND_GIFT", "data": {"giftName": "辣条"}});
        let bytes = serde_json::to_vec(&json).expect("测试 JSON 应可序列化");
        assert_eq!(parse_danmu_msg(&bytes), Ok(None));

        // 仅前缀相同但不是协议变体的 cmd 也不能被误认。
        let json = serde_json::json!({"cmd": "DANMU_MSGX", "info": []});
        let bytes = serde_json::to_vec(&json).expect("测试 JSON 应可序列化");
        assert_eq!(parse_danmu_msg(&bytes), Ok(None));
    }

    #[test]
    fn invalid_json_errors() {
        assert!(matches!(
            parse_danmu_msg(b"{not json"),
            Err(DanmakuProtocolError::InvalidJson(_))
        ));
        assert!(matches!(
            parse_danmu_msg(&[0xFF, 0xFE]),
            Err(DanmakuProtocolError::InvalidJson(_))
        ));
    }

    #[test]
    fn missing_cmd_errors() {
        let bytes = serde_json::to_vec(&serde_json::json!({"info": []})).expect("应可序列化");
        assert_eq!(
            parse_danmu_msg(&bytes),
            Err(DanmakuProtocolError::MissingField("cmd"))
        );
    }

    #[test]
    fn missing_text_errors() {
        let json = serde_json::json!({
            "cmd": "DANMU_MSG",
            "info": [[0, 1, 25, 16777215, 1726799123456i64]],
        });
        let bytes = serde_json::to_vec(&json).expect("应可序列化");
        assert_eq!(
            parse_danmu_msg(&bytes),
            Err(DanmakuProtocolError::MissingField("info[1]"))
        );
    }

    #[test]
    fn missing_user_errors() {
        let json = serde_json::json!({
            "cmd": "DANMU_MSG",
            "info": [[0, 1, 25, 16777215, 1726799123456i64], "晚上好"],
        });
        let bytes = serde_json::to_vec(&json).expect("应可序列化");
        assert_eq!(
            parse_danmu_msg(&bytes),
            Err(DanmakuProtocolError::MissingField("info[2]"))
        );
    }

    #[test]
    fn user_field_short_errors() {
        let mut value = danmu_value();
        value["info"][2] = serde_json::json!([123456]);
        let bytes = serde_json::to_vec(&value).expect("应可序列化");
        assert_eq!(
            parse_danmu_msg(&bytes),
            Err(DanmakuProtocolError::MissingField("info[2][1]"))
        );
    }

    #[test]
    fn missing_color_errors() {
        let mut value = danmu_value();
        value["info"][0] = serde_json::json!([0, 1, 25]);
        let bytes = serde_json::to_vec(&value).expect("应可序列化");
        assert_eq!(
            parse_danmu_msg(&bytes),
            Err(DanmakuProtocolError::MissingField("info[0][3]"))
        );
    }

    #[test]
    fn missing_timestamp_errors() {
        let mut value = danmu_value();
        value["info"][0] = serde_json::json!([0, 1, 25, 16777215]);
        let bytes = serde_json::to_vec(&value).expect("应可序列化");
        assert_eq!(
            parse_danmu_msg(&bytes),
            Err(DanmakuProtocolError::MissingField("info[0][4]"))
        );
    }

    #[test]
    fn invalid_field_types_error() {
        let mut value = danmu_value();
        value["info"] = serde_json::json!("不是数组");
        let bytes = serde_json::to_vec(&value).expect("应可序列化");
        assert_eq!(
            parse_danmu_msg(&bytes),
            Err(DanmakuProtocolError::InvalidField("info"))
        );

        let mut value = danmu_value();
        value["info"][0][3] = serde_json::json!("白色");
        let bytes = serde_json::to_vec(&value).expect("应可序列化");
        assert_eq!(
            parse_danmu_msg(&bytes),
            Err(DanmakuProtocolError::InvalidField("info[0][3]"))
        );

        let mut value = danmu_value();
        value["info"][0][3] = serde_json::json!(u64::from(u32::MAX) + 1);
        let bytes = serde_json::to_vec(&value).expect("应可序列化");
        assert_eq!(
            parse_danmu_msg(&bytes),
            Err(DanmakuProtocolError::InvalidField("info[0][3]"))
        );
    }

    #[test]
    fn parse_decoded_notification_body() {
        let encoded = encode_packet(OP_NOTIFICATION, DANMU_JSON.as_bytes());
        let packets = decode_packets(&encoded).expect("通知包应能解出");
        assert_eq!(packets.len(), 1);
        let message = parse_danmu_msg(&packets[0].body)
            .expect("合法弹幕不应报错")
            .expect("DANMU_MSG 应返回 Some");
        assert_eq!(message.user, "用户A");
    }
}
