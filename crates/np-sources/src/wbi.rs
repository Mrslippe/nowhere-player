//! B 站 `wbi` 签名（纯函数）。
//!
//! 步骤（任一步错都会得到 -403）：
//! 1. 从 nav 接口的 `wbi_img.img_url` / `sub_url` 中截取 32 位 key；
//! 2. `mixin_key = 打乱(img_key + sub_key)[..32]`；
//! 3. 参数按 key 升序、value 过滤 `!'()*`、百分号编码（大写、空格 `%20`）；
//! 4. `w_rid = md5(query + mixin_key)`，把 `wts` 与 `w_rid` 追加到 query。
//!
//! 本模块不涉及网络与时钟：`wts` 由调用方传入，固定向量因此可复现。

use std::fmt::Write as _;

/// 打乱映射表，共 64 项；前 32 项决定 mixin key（顺序不可改）。
pub const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

/// 从 `wbi_img` 的图片 URL 中截取 32 位 hex key。
///
/// 例：`https://i0.hdslb.com/bfs/wbi/<key>.png` -> `<key>`。
/// 只接受 32 位十六进制，避免把异常 URL 当成 key 继续算签名。
pub fn key_from_url(url: &str) -> Option<&str> {
    let path = url.split(['?', '#']).next()?;
    let file = path.rsplit('/').next()?;
    let stem = file.split('.').next()?;
    if stem.len() == 32 && stem.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(stem)
    } else {
        None
    }
}

/// 由 `img_key` + `sub_key` 生成 32 位 mixin key。
pub fn mixin_key(img_key: &str, sub_key: &str) -> String {
    let raw: Vec<char> = img_key.chars().chain(sub_key.chars()).collect();
    MIXIN_KEY_ENC_TAB
        .iter()
        .take(32)
        .filter_map(|&index| raw.get(index).copied())
        .collect()
}

/// 对参数签名，返回可直接拼到 URL 后的 query（含 `wts` 与 `w_rid`）。
///
/// `wts` 显式传入而不是在函数内取系统时间，保证测试向量可复现；
/// 生产调用方传入当前 Unix 秒。调用方自带的 `wts` / `w_rid` 会被覆盖。
pub fn sign(params: &[(String, String)], mixin_key: &str, wts: u64) -> String {
    let mut signed: Vec<(String, String)> = params
        .iter()
        .filter(|(key, _)| key != "wts" && key != "w_rid")
        .cloned()
        .collect();
    signed.push(("wts".to_string(), wts.to_string()));
    signed.sort_by(|left, right| left.0.cmp(&right.0));

    let mut query = String::new();
    for (index, (key, value)) in signed.iter().enumerate() {
        if index > 0 {
            query.push('&');
        }
        query.push_str(&encode_component(key));
        query.push('=');
        query.push_str(&encode_component(&filter_value(value)));
    }

    let mut hasher_input = String::with_capacity(query.len() + mixin_key.len());
    hasher_input.push_str(&query);
    hasher_input.push_str(mixin_key);
    let rid = format!("{:x}", md5::compute(hasher_input.as_bytes()));

    let _ = write!(query, "&w_rid={rid}");
    query
}

/// 百分号编码：等价于 `encodeURIComponent`，空格编码为 `%20`，字母大写。
fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

/// wbi 要求过滤 value 中的 `!'()*`（`encodeURIComponent` 本来不转义它们）。
fn filter_value(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !matches!(ch, '!' | '\'' | '(' | ')' | '*'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMG_KEY: &str = "7cd084941338484aae1ad9425b84077c";
    const SUB_KEY: &str = "4932caff0ff746eab6f01bf08b70ac45";
    /// 文档固定时间戳，保证向量稳定。
    const WTS: u64 = 1_702_204_169;

    fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn mixin_key_matches_published_vector() {
        assert_eq!(
            mixin_key(IMG_KEY, SUB_KEY),
            "ea1db124af3c7062474693fa704f4ff8"
        );
    }

    #[test]
    fn key_from_url_extracts_stem_and_rejects_garbage() {
        assert_eq!(
            key_from_url("https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png"),
            Some("7cd084941338484aae1ad9425b84077c")
        );
        assert_eq!(key_from_url("https://example.com/wbi/not-a-key.png"), None);
        assert_eq!(key_from_url(""), None);
    }

    /// 官方文档给出的固定向量（Rust demo 的测试用例）。
    #[test]
    fn sign_matches_published_vector() {
        let signed = sign(
            &params(&[("foo", "114"), ("bar", "514"), ("zab", "1919810")]),
            &mixin_key(IMG_KEY, SUB_KEY),
            WTS,
        );
        assert_eq!(
            signed,
            "bar=514&foo=114&wts=1702204169&zab=1919810&w_rid=8f6f2b5b3d485fe1886cec6a0be8c5d4"
        );
    }

    #[test]
    fn sign_encodes_chinese_and_sorts_parameters() {
        let signed = sign(
            &params(&[
                ("search_type", "video"),
                ("page_size", "3"),
                ("keyword", "周杰伦"),
                ("page", "1"),
            ]),
            &mixin_key(IMG_KEY, SUB_KEY),
            WTS,
        );
        assert_eq!(
            signed,
            "keyword=%E5%91%A8%E6%9D%B0%E4%BC%A6&page=1&page_size=3&search_type=video&\
             wts=1702204169&w_rid=f8c40bb6cfde70f5d82dee7b328263d8"
        );
    }

    #[test]
    fn sign_filters_special_chars_and_overrides_supplied_wts() {
        let signed = sign(
            &params(&[
                ("keyword", "a!b'c(d)e*f&g=中文 空格"),
                ("search_type", "video"),
                ("wts", "999"),
                ("w_rid", "deadbeef"),
            ]),
            &mixin_key(IMG_KEY, SUB_KEY),
            WTS,
        );
        assert_eq!(
            signed,
            "keyword=abcdef%26g%3D%E4%B8%AD%E6%96%87%20%E7%A9%BA%E6%A0%BC&\
             search_type=video&wts=1702204169&w_rid=262aa75aaffa4bd3e173b2ae0405245f"
        );
    }
}
