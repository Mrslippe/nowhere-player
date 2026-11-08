//! 主题文件模型与校验（T-0029 §6.1）。
//!
//! 用户主题是**不可信输入**：`id` 会变成文件名，`tokens` 会变成 `<html>` 上的
//! inline CSS 变量。本模块只做纯逻辑校验（不碰文件系统、不碰 Tauri），
//! 因此可以脱离桌面运行时单测。
//!
//! 两层模型见 ADR-0003：
//! - `tokens`（颜色）**完全开放**，缺项用 [`ThemeTokens::default`]（= graphite）补；
//! - `skin`（结构）**有限枚举**，未知值回落到 `plain`；
//! - `contract` 是文件版本号，不等于 [`THEME_CONTRACT`] 时**加载但告警**。
//!
//! 颜色只接受 `#RGB` / `#RRGGBB` / `#RRGGBBAA` / `rgba(...)`；其它形式视为
//! 非法并跳过该项（不是整份主题失败，见任务卡 §6.1）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 当前主题文件契约版本。
pub const THEME_CONTRACT: u32 = 1;

/// `id` 的最大长度（字节；也是文件名主干的长度上限）。
pub const MAX_THEME_ID_LEN: usize = 48;

/// `name` 的最大长度（字符数，中文主题名不被字节数误杀）。
pub const MAX_THEME_NAME_CHARS: usize = 64;

/// 错误码：`id` 缺失或非法。
pub const E_THEME_ID: &str = "E_THEME_ID";
/// 错误码：缺少 `contract`。
pub const E_THEME_CONTRACT: &str = "E_THEME_CONTRACT";
/// 错误码：`name` 超长。
pub const E_THEME_NAME: &str = "E_THEME_NAME";
/// 错误码：JSON 解析失败。
pub const E_THEME_JSON: &str = "E_THEME_JSON";
/// 错误码：主题文件读写失败。
pub const E_THEME_IO: &str = "E_THEME_IO";
/// 错误码：主题不存在。
pub const E_THEME_NOT_FOUND: &str = "E_THEME_NOT_FOUND";

/// 所有可配置的**颜色** token（camelCase，与主题文件一致）。
///
/// 结构 token（圆角 / 间距 / 字体 / 阴影）不在这里：它们属于 `skin`，
/// 写代码而不是写配置（ADR-0003 §2）。
pub const THEME_TOKEN_NAMES: [&str; 14] = [
    "bg",
    "bgElev",
    "bgHover",
    "fg",
    "fgMuted",
    "fgFaint",
    "accent",
    "accentSoft",
    "accentFg",
    "danger",
    "warn",
    "success",
    "line",
    "lineStrong",
];

/// 结构皮肤（v1 只有两个：默认 plain、磷光 phosphor）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Skin {
    /// 默认皮肤（现有观感）。
    #[default]
    Plain,
    /// 磷光皮肤（T-0028）。
    Phosphor,
}

impl Skin {
    /// 解析 `skin` 字段；未知值返回 `None`（由调用方回落 `plain` 并告警）。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "plain" => Some(Self::Plain),
            "phosphor" => Some(Self::Phosphor),
            _ => None,
        }
    }

    /// 序列化用的字面量。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Phosphor => "phosphor",
        }
    }
}

/// 14 个颜色 token 的完整集合；`Default` 就是 graphite（内置默认值）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeTokens {
    /// 背景。
    pub bg: String,
    /// 抬升背景（卡片 / 面板）。
    pub bg_elev: String,
    /// hover 背景。
    pub bg_hover: String,
    /// 正文前景。
    pub fg: String,
    /// 次要前景。
    pub fg_muted: String,
    /// 更弱的前景（占位符 / 分隔）。
    pub fg_faint: String,
    /// 强调色。
    pub accent: String,
    /// 强调色的半透明底。
    pub accent_soft: String,
    /// 强调色上的前景（按钮文字）。
    pub accent_fg: String,
    /// 错误色。
    pub danger: String,
    /// 警告色。
    pub warn: String,
    /// 成功色。
    pub success: String,
    /// 细线。
    pub line: String,
    /// 强线。
    pub line_strong: String,
}

impl Default for ThemeTokens {
    /// graphite 调色板（与 `apps/web/src/design/themes.css` 的 `:root` 一致）。
    fn default() -> Self {
        Self {
            bg: "#0b0c0e".to_owned(),
            bg_elev: "#141619".to_owned(),
            bg_hover: "#1c1f24".to_owned(),
            fg: "#e8eaed".to_owned(),
            fg_muted: "#8b9096".to_owned(),
            fg_faint: "#4a4f55".to_owned(),
            accent: "#7c9ef5".to_owned(),
            accent_soft: "rgba(124, 158, 245, 0.14)".to_owned(),
            accent_fg: "#0b0c0e".to_owned(),
            danger: "#f2707a".to_owned(),
            warn: "#e8b45c".to_owned(),
            success: "#7fc79a".to_owned(),
            line: "rgba(255, 255, 255, 0.07)".to_owned(),
            line_strong: "rgba(255, 255, 255, 0.14)".to_owned(),
        }
    }
}

impl ThemeTokens {
    /// 按 token 名读写；未知名字返回 `false` / `None`。
    pub fn set(&mut self, name: &str, value: &str) -> bool {
        let Some(slot) = self.slot_mut(name) else {
            return false;
        };
        *slot = value.to_owned();
        true
    }

    /// 按 token 名读取；未知名字返回 `None`。
    pub fn get(&self, name: &str) -> Option<&str> {
        let slot = match name {
            "bg" => &self.bg,
            "bgElev" => &self.bg_elev,
            "bgHover" => &self.bg_hover,
            "fg" => &self.fg,
            "fgMuted" => &self.fg_muted,
            "fgFaint" => &self.fg_faint,
            "accent" => &self.accent,
            "accentSoft" => &self.accent_soft,
            "accentFg" => &self.accent_fg,
            "danger" => &self.danger,
            "warn" => &self.warn,
            "success" => &self.success,
            "line" => &self.line,
            "lineStrong" => &self.line_strong,
            _ => return None,
        };
        Some(slot)
    }

    fn slot_mut(&mut self, name: &str) -> Option<&mut String> {
        match name {
            "bg" => Some(&mut self.bg),
            "bgElev" => Some(&mut self.bg_elev),
            "bgHover" => Some(&mut self.bg_hover),
            "fg" => Some(&mut self.fg),
            "fgMuted" => Some(&mut self.fg_muted),
            "fgFaint" => Some(&mut self.fg_faint),
            "accent" => Some(&mut self.accent),
            "accentSoft" => Some(&mut self.accent_soft),
            "accentFg" => Some(&mut self.accent_fg),
            "danger" => Some(&mut self.danger),
            "warn" => Some(&mut self.warn),
            "success" => Some(&mut self.success),
            "line" => Some(&mut self.line),
            "lineStrong" => Some(&mut self.line_strong),
            _ => None,
        }
    }
}

/// 主题文件反序列化目标：字段全部可选，校验交给 [`ThemeSpec::from_raw`]。
///
/// 不直接反序列化成 [`ThemeSpec`] 是为了把「缺 contract」「未知 skin」
/// 「颜色非法」分别变成可读的错误 / 告警，而不是一句 serde 报错。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RawTheme {
    /// 主题 id（会作为文件名）。
    pub id: Option<String>,
    /// 显示名。
    pub name: Option<String>,
    /// 契约版本。
    pub contract: Option<u32>,
    /// 皮肤名（字符串，未知值回落 `plain`）。
    pub skin: Option<String>,
    /// 颜色 token 表（未知键忽略，非法值跳过）。
    pub tokens: Option<BTreeMap<String, String>>,
}

/// 校验通过的主题。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSpec {
    /// 主题 id（同时是文件名主干）。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 契约版本。
    pub contract: u32,
    /// 结构皮肤。
    pub skin: Skin,
    /// 完整颜色 token（缺项已补默认值）。
    pub tokens: ThemeTokens,
    /// 加载时产生的告警（未知 skin、非法颜色等），不阻止应用。
    pub warnings: Vec<String>,
}

impl ThemeSpec {
    /// 从原始字段校验出可用的主题。
    ///
    /// 失败只针对「无法安全使用」的情况：缺 / 非法 id、缺 contract、超长 name。
    /// 其余一切（未知 skin、非法颜色、未知 token、非 1 的 contract）都降级 + 告警。
    pub fn from_raw(raw: RawTheme) -> Result<Self, ThemeError> {
        let id = raw
            .id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                ThemeError::new(E_THEME_ID, "主题缺少 id")
                    .with_hint("id 会作为文件名，只能包含字母、数字、- _ .，且不能是 . 或包含 ..")
            })?
            .to_owned();
        validate_theme_id(&id)?;

        let contract = raw.contract.ok_or_else(|| {
            ThemeError::new(E_THEME_CONTRACT, "主题缺少 contract 字段")
                .with_hint(format!("当前契约版本是 {THEME_CONTRACT}"))
        })?;

        let mut warnings = Vec::new();
        if contract != THEME_CONTRACT {
            warnings.push(format!(
                "contract {contract} 与当前版本 {THEME_CONTRACT} 不一致，已按当前版本加载"
            ));
        }

        let name = raw
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&id)
            .to_owned();
        if name.chars().count() > MAX_THEME_NAME_CHARS {
            return Err(ThemeError::new(
                E_THEME_NAME,
                format!("主题名过长（上限 {MAX_THEME_NAME_CHARS} 个字符）"),
            ));
        }

        let skin = match raw.skin.as_deref() {
            None => Skin::Plain,
            Some(value) => Skin::parse(value).unwrap_or_else(|| {
                warnings.push(format!("未知 skin：{value}，已回落 plain"));
                Skin::Plain
            }),
        };

        let defaults = Self::default().tokens;
        let mut tokens = ThemeTokens::default();
        for (key, value) in raw.tokens.unwrap_or_default() {
            let Some(fallback) = defaults.get(&key).map(str::to_owned) else {
                warnings.push(format!("未知 token：{key}，已忽略"));
                continue;
            };
            let candidate = value.trim();
            if is_valid_theme_color(candidate) {
                tokens.set(&key, candidate);
            } else {
                tokens.set(&key, &fallback);
                warnings.push(format!(
                    "tokens.{key} 颜色非法（{value}），已用默认值 {fallback}"
                ));
            }
        }

        Ok(Self {
            id,
            name,
            contract,
            skin,
            tokens,
            warnings,
        })
    }

    /// 导出成主题文件（供 `theme.exportBuiltin` 给用户改）。
    ///
    /// 不依赖 serde_json：字段是固定集合，手写一份稳定的 pretty JSON 即可，
    /// 输出能被 [`RawTheme`] 原样反序列化（有单测保证）。
    pub fn to_json_pretty(&self) -> String {
        let mut out = String::new();
        out.push_str("{\n");
        out.push_str(&format!("  \"id\": {},\n", json_string(&self.id)));
        out.push_str(&format!("  \"name\": {},\n", json_string(&self.name)));
        out.push_str(&format!("  \"contract\": {},\n", self.contract));
        out.push_str(&format!(
            "  \"skin\": {},\n",
            json_string(self.skin.as_str())
        ));
        out.push_str("  \"tokens\": {\n");
        let last = THEME_TOKEN_NAMES.len().saturating_sub(1);
        for (index, name) in THEME_TOKEN_NAMES.iter().enumerate() {
            let value = self.tokens.get(name).unwrap_or_default();
            let suffix = if index == last { "" } else { "," };
            out.push_str(&format!(
                "    {}: {}{suffix}\n",
                json_string(name),
                json_string(value)
            ));
        }
        out.push_str("  }\n");
        out.push_str("}\n");
        out
    }
}

impl Default for ThemeSpec {
    /// 一份合法的默认主题（graphite），用于兜底与单测。
    fn default() -> Self {
        Self {
            id: "graphite".to_owned(),
            name: "石墨".to_owned(),
            contract: THEME_CONTRACT,
            skin: Skin::Plain,
            tokens: ThemeTokens::default(),
            warnings: Vec::new(),
        }
    }
}

/// 校验 id 是否是安全的文件名主干。
///
/// 规则：非空、长度 ≤ [`MAX_THEME_ID_LEN`]、只允许 ASCII 字母 / 数字 / `-` / `_` / `.`，
/// **不允许**路径分隔符、`..`、首尾 `.`。满足这些条件时 `themes/<id>.json`
/// 不可能逃出主题目录（T-0029 §11 风险 2）。
pub fn validate_theme_id(id: &str) -> Result<(), ThemeError> {
    if id.is_empty() {
        return Err(ThemeError::new(E_THEME_ID, "主题 id 不能为空"));
    }
    if id.len() > MAX_THEME_ID_LEN {
        return Err(ThemeError::new(
            E_THEME_ID,
            format!("主题 id 过长（上限 {MAX_THEME_ID_LEN} 字节）"),
        ));
    }
    let safe = id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if !safe || id.contains("..") || id.starts_with('.') || id.ends_with('.') {
        return Err(ThemeError::new(E_THEME_ID, format!("主题 id 非法：{id}"))
            .with_hint("id 会作为文件名，只能包含字母、数字、- _ .，且不能是 . 或包含 .."));
    }
    Ok(())
}

/// 颜色值是否是允许的格式（`#RGB` / `#RRGGBB` / `#RRGGBBAA` / `rgba(...)`）。
pub fn is_valid_theme_color(value: &str) -> bool {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let width_ok = matches!(hex.len(), 3 | 6 | 8);
        return width_ok && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    is_valid_rgba(value)
}

fn is_valid_rgba(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let Some(inner) = lower
        .strip_prefix("rgba(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return false;
    }
    let channels_ok = parts[..3].iter().all(|part| part.parse::<u8>().is_ok());
    let alpha_ok = parts[3]
        .parse::<f64>()
        .is_ok_and(|alpha| (0.0..=1.0).contains(&alpha));
    channels_ok && alpha_ok
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

/// 主题来源：内置（随包分发）还是用户文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeSource {
    /// 内置主题（`apps/web/src/design/themes.css`）。
    Builtin,
    /// `<appdata>/themes/<id>.json`。
    User,
}

/// `theme.list` 的一项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeListEntry {
    /// 主题 id。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 结构皮肤。
    pub skin: Skin,
    /// 来源。
    pub source: ThemeSource,
}

/// `theme.apply` 的回执：前端据此写 inline CSS 变量。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeApplyReply {
    /// 主题 id。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 结构皮肤。
    pub skin: Skin,
    /// 来源。
    pub source: ThemeSource,
    /// 完整颜色 token（缺项已补默认值）。
    pub tokens: ThemeTokens,
    /// 加载告警（不阻止应用）。
    pub warnings: Vec<String>,
}

impl ThemeApplyReply {
    /// 由校验通过的主题生成回执。
    pub fn from_spec(spec: ThemeSpec, source: ThemeSource) -> Self {
        Self {
            id: spec.id,
            name: spec.name,
            skin: spec.skin,
            source,
            tokens: spec.tokens,
            warnings: spec.warnings,
        }
    }

    /// 由内置主题生成回执。
    pub fn from_builtin(theme: &BuiltinTheme) -> Self {
        Self::from_spec(theme.to_spec(), ThemeSource::Builtin)
    }
}

/// 内置主题：id / 显示名 / 皮肤 / 调色板。
///
/// 调色板与 `apps/web/src/design/themes.css` 一一对应（单测会对比源码，防止漂移）；
/// Rust 侧需要它生成 `theme.exportBuiltin` 的样例文件。
#[derive(Debug, Clone, Copy)]
pub struct BuiltinTheme {
    /// 主题 id（与 `[data-theme]` 取值一致）。
    pub id: &'static str,
    /// 显示名。
    pub name: &'static str,
    /// 结构皮肤。
    pub skin: Skin,
    /// 14 个颜色 token（camelCase 名 → 色值）。
    pub tokens: [(&'static str, &'static str); 14],
}

impl BuiltinTheme {
    /// 按 id 查内置主题。
    pub fn find(id: &str) -> Option<&'static Self> {
        BUILTIN_THEMES.iter().find(|theme| theme.id == id)
    }

    /// 展开成完整的 [`ThemeSpec`]。
    pub fn to_spec(self) -> ThemeSpec {
        let mut tokens = ThemeTokens::default();
        for (name, value) in self.tokens {
            tokens.set(name, value);
        }
        ThemeSpec {
            id: self.id.to_owned(),
            name: self.name.to_owned(),
            contract: THEME_CONTRACT,
            skin: self.skin,
            tokens,
            warnings: Vec::new(),
        }
    }

    /// 展开成 `theme.list` 的一项。
    pub fn entry(self) -> ThemeListEntry {
        ThemeListEntry {
            id: self.id.to_owned(),
            name: self.name.to_owned(),
            skin: self.skin,
            source: ThemeSource::Builtin,
        }
    }
}

/// 四个内置主题（顺序不敏感；`theme.list` 会按 id 排序）。
pub static BUILTIN_THEMES: [BuiltinTheme; 4] = [
    BuiltinTheme {
        id: "graphite",
        name: "石墨",
        skin: Skin::Plain,
        tokens: [
            ("bg", "#0b0c0e"),
            ("bgElev", "#141619"),
            ("bgHover", "#1c1f24"),
            ("fg", "#e8eaed"),
            ("fgMuted", "#8b9096"),
            ("fgFaint", "#4a4f55"),
            ("accent", "#7c9ef5"),
            ("accentSoft", "rgba(124, 158, 245, 0.14)"),
            ("accentFg", "#0b0c0e"),
            ("danger", "#f2707a"),
            ("warn", "#e8b45c"),
            ("success", "#7fc79a"),
            ("line", "rgba(255, 255, 255, 0.07)"),
            ("lineStrong", "rgba(255, 255, 255, 0.14)"),
        ],
    },
    BuiltinTheme {
        id: "amber",
        name: "琥珀",
        skin: Skin::Plain,
        tokens: [
            ("bg", "#0c0a07"),
            ("bgElev", "#16120c"),
            ("bgHover", "#221c13"),
            ("fg", "#f2e9da"),
            ("fgMuted", "#af9f86"),
            ("fgFaint", "#5c5245"),
            ("accent", "#f0a93c"),
            ("accentSoft", "rgba(240, 169, 60, 0.14)"),
            ("accentFg", "#1a1206"),
            ("danger", "#f2707a"),
            ("warn", "#f2c94c"),
            ("success", "#8bcb8f"),
            ("line", "rgba(255, 222, 180, 0.08)"),
            ("lineStrong", "rgba(255, 222, 180, 0.16)"),
        ],
    },
    BuiltinTheme {
        id: "ice",
        name: "冰",
        skin: Skin::Plain,
        tokens: [
            ("bg", "#070b10"),
            ("bgElev", "#0e151d"),
            ("bgHover", "#16202a"),
            ("fg", "#e2f0fa"),
            ("fgMuted", "#8aa3b5"),
            ("fgFaint", "#46596a"),
            ("accent", "#61c7e8"),
            ("accentSoft", "rgba(97, 199, 232, 0.14)"),
            ("accentFg", "#041017"),
            ("danger", "#f27e8a"),
            ("warn", "#e8b45c"),
            ("success", "#7fc79a"),
            ("line", "rgba(190, 225, 255, 0.08)"),
            ("lineStrong", "rgba(190, 225, 255, 0.16)"),
        ],
    },
    BuiltinTheme {
        id: "phosphor",
        name: "磷光",
        skin: Skin::Phosphor,
        tokens: [
            ("bg", "#0a0806"),
            ("bgElev", "#120e08"),
            ("bgHover", "#1b140c"),
            ("fg", "#ffb454"),
            ("fgMuted", "#a87b3f"),
            ("fgFaint", "#5c4526"),
            ("accent", "#ffb454"),
            ("accentSoft", "rgba(255, 180, 84, 0.14)"),
            ("accentFg", "#0a0806"),
            ("danger", "#e05b3c"),
            ("warn", "#ffc978"),
            ("success", "#ffd18a"),
            ("line", "rgba(255, 180, 84, 0.14)"),
            ("lineStrong", "rgba(255, 180, 84, 0.28)"),
        ],
    },
];

/// 主题相关的失败（`{ code, message, hint? }`，与终端 error 原语同形）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeError {
    /// 错误码，如 `E_THEME_ID`。
    pub code: String,
    /// 面向用户的说明。
    pub message: String,
    /// 可选的修复建议。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl ThemeError {
    /// 构造一个没有 hint 的错误。
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            hint: None,
        }
    }

    /// 补上修复建议。
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ThemeError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<ThemeSpec, ThemeError> {
        let raw: RawTheme = serde_json::from_str(json).expect("测试用例必须是合法 JSON");
        ThemeSpec::from_raw(raw)
    }

    const NORMAL: &str = r##"{
        "id": "my-theme",
        "name": "我的主题",
        "contract": 1,
        "skin": "plain",
        "tokens": {
            "bg": "#14100E",
            "bgElev": "#1E1916",
            "fg": "#E8DCC8",
            "fgMuted": "#A08D76",
            "accent": "#D98A3D",
            "danger": "#C6402F",
            "line": "rgba(255,255,255,.07)"
        }
    }"##;

    #[test]
    fn normal_theme_is_accepted() {
        let spec = parse(NORMAL).expect("正常主题");

        assert_eq!(spec.id, "my-theme");
        assert_eq!(spec.name, "我的主题");
        assert_eq!(spec.contract, 1);
        assert_eq!(spec.skin, Skin::Plain);
        assert_eq!(spec.tokens.bg, "#14100E");
        assert_eq!(spec.tokens.line, "rgba(255,255,255,.07)");
        assert!(spec.warnings.is_empty());
    }

    #[test]
    fn missing_contract_is_rejected() {
        let error = parse(r##"{"id":"x","name":"x"}"##).expect_err("缺 contract");
        assert_eq!(error.code, E_THEME_CONTRACT);
    }

    #[test]
    fn contract_two_loads_with_warning() {
        let spec = parse(r##"{"id":"x","contract":2}"##).expect("contract 2 仍加载");

        assert_eq!(spec.contract, 2);
        assert!(
            spec.warnings.iter().any(|w| w.contains("contract 2")),
            "应给出 contract 告警：{:?}",
            spec.warnings
        );
    }

    #[test]
    fn unknown_skin_falls_back_to_plain() {
        let spec = parse(r##"{"id":"x","contract":1,"skin":"neon"}"##).expect("未知 skin");

        assert_eq!(spec.skin, Skin::Plain);
        assert!(spec.warnings.iter().any(|w| w.contains("neon")));
    }

    #[test]
    fn phosphor_skin_is_kept() {
        let spec = parse(r##"{"id":"x","contract":1,"skin":"phosphor"}"##).expect("磷光皮肤");
        assert_eq!(spec.skin, Skin::Phosphor);
    }

    #[test]
    fn missing_tokens_are_filled_with_graphite() {
        let spec = parse(r##"{"id":"x","contract":1}"##).expect("无 tokens");

        assert_eq!(spec.tokens, ThemeTokens::default());
        assert_eq!(spec.tokens.bg, "#0b0c0e");
    }

    #[test]
    fn invalid_color_is_skipped() {
        let spec = parse(
            r##"{"id":"x","contract":1,"tokens":{"bg":"#123456","fg":"rebeccapurple","accent":"#12"}}"##,
        )
        .expect("非法颜色不阻止加载");

        assert_eq!(spec.tokens.bg, "#123456");
        assert_eq!(spec.tokens.fg, ThemeTokens::default().fg);
        assert_eq!(spec.tokens.accent, ThemeTokens::default().accent);
        assert_eq!(
            spec.warnings
                .iter()
                .filter(|w| w.contains("颜色非法"))
                .count(),
            2
        );
    }

    #[test]
    fn unknown_token_is_ignored() {
        let spec =
            parse(r##"{"id":"x","contract":1,"tokens":{"glow":"#fff"}}"##).expect("未知 token");
        assert!(spec.warnings.iter().any(|w| w.contains("glow")));
    }

    #[test]
    fn empty_id_is_rejected() {
        let error = parse(r##"{"id":"   ","contract":1}"##).expect_err("空 id");
        assert_eq!(error.code, E_THEME_ID);
    }

    #[test]
    fn path_traversal_ids_are_rejected() {
        for id in [
            "../evil",
            "a/b",
            "a\\b",
            "..",
            ".hidden",
            "a.",
            "id with space",
        ] {
            let json = format!(r#"{{"id":"{}","contract":1}}"#, id.replace('\\', "\\\\"));
            let error = parse(&json).expect_err(id);
            assert_eq!(error.code, E_THEME_ID, "id 被接受：{id:?}");
        }
    }

    #[test]
    fn too_long_name_is_rejected() {
        let name = "长".repeat(MAX_THEME_NAME_CHARS + 1);
        let json = format!(r#"{{"id":"x","contract":1,"name":"{name}"}}"#);
        let error = parse(&json).expect_err("超长 name");
        assert_eq!(error.code, E_THEME_NAME);
    }

    #[test]
    fn missing_name_falls_back_to_id() {
        let spec = parse(r##"{"id":"x","contract":1}"##).expect("无 name");
        assert_eq!(spec.name, "x");
    }

    #[test]
    fn color_formats_are_strict() {
        for value in [
            "#fff",
            "#FFF",
            "#a1b2c3",
            "#a1b2c3d4",
            "rgba(0, 0, 0, 0.5)",
            "rgba(255,255,255,.07)",
        ] {
            assert!(is_valid_theme_color(value), "应接受：{value}");
        }

        for value in [
            "",
            "#ff",
            "#ffff",
            "#gggggg",
            "rgb(0, 0, 0)",
            "rgba(256, 0, 0, 1)",
            "rgba(0, 0, 0)",
            "rgba(0, 0, 0, 1.5)",
            "red",
        ] {
            assert!(!is_valid_theme_color(value), "应拒绝：{value}");
        }
    }

    #[test]
    fn export_json_round_trips() {
        let spec = parse(NORMAL).expect("正常主题");
        let json = spec.to_json_pretty();
        let reparsed = parse(&json).expect("导出文件必须能被重新加载");

        assert_eq!(reparsed.id, spec.id);
        assert_eq!(reparsed.name, spec.name);
        assert_eq!(reparsed.tokens, spec.tokens);
        assert!(reparsed.warnings.is_empty());
    }

    #[test]
    fn export_escapes_quotes_and_control_characters() {
        let spec = ThemeSpec {
            id: "quote".to_owned(),
            name: "含\"引号\"\n换行".to_owned(),
            ..ThemeSpec::default()
        };
        let json = spec.to_json_pretty();
        assert!(json.contains("\\\"引号\\\""));
        assert!(json.contains("\\n"));

        let reparsed = parse(&json).expect("转义后仍合法");
        assert_eq!(reparsed.name, "含\"引号\"\n换行");
    }

    /// 把 `themes.css` 里某个主题块的内容抠出来（测试专用，不做通用 CSS 解析）。
    fn css_block(theme: &str) -> &'static str {
        let css = include_str!("../../../apps/web/src/design/themes.css");
        let selector = format!("[data-theme=\"{theme}\"]");
        let select = css.find(&selector).expect("themes.css 里应有该主题");
        let open = css[select..].find('{').expect("缺少左花括号") + select;
        let close = css[open..].find('}').expect("缺少右花括号") + open;
        &css[open + 1..close]
    }

    /// camelCase token → CSS 自定义属性名（`bgElev` → `--bg-elev`）。
    fn css_var_name(token: &str) -> String {
        let mut name = String::from("--");
        for character in token.chars() {
            if character.is_ascii_uppercase() {
                name.push('-');
                name.push(character.to_ascii_lowercase());
            } else {
                name.push(character);
            }
        }
        name
    }

    #[test]
    fn builtin_palettes_match_design_themes_css() {
        for theme in BUILTIN_THEMES.iter() {
            let block = css_block(theme.id);
            for (token, value) in theme.tokens {
                let declaration = format!("{}: {};", css_var_name(token), value);
                assert!(
                    block.contains(&declaration),
                    "内置主题 {} 的 {} 与 themes.css 不一致，期望：{}",
                    theme.id,
                    token,
                    declaration
                );
            }
        }
    }

    #[test]
    fn builtin_themes_cover_the_four_ids() {
        let ids: Vec<&str> = BUILTIN_THEMES.iter().map(|theme| theme.id).collect();
        assert_eq!(ids, ["graphite", "amber", "ice", "phosphor"]);
        assert_eq!(
            BuiltinTheme::find("phosphor").map(|theme| theme.skin),
            Some(Skin::Phosphor)
        );
        assert_eq!(
            BuiltinTheme::find("graphite").map(|theme| theme.skin),
            Some(Skin::Plain)
        );
        assert!(BuiltinTheme::find("nope").is_none());
    }
}
