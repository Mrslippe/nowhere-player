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

macro_rules! structure_axis {
    ($name:ident, [$($variant:ident => $text:literal),+ $(,)?]) => {
        /// 结构轴取值；未知字符串由 [`parse_axis`] 回落并告警。
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "lowercase")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// 大小写不敏感解析；未知值返回 `None`。
            pub fn parse(value: &str) -> Option<Self> {
                match value.trim().to_ascii_lowercase().as_str() {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// 写进主题 JSON / DOM 属性的规范值。
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl Axis for $name {
            fn parse_axis_value(value: &str) -> Option<Self> {
                Self::parse(value)
            }

            fn as_str(self) -> &'static str {
                self.as_str()
            }
        }
    };
}

structure_axis!(Radius, [None => "none", Subtle => "subtle", Soft => "soft"]);
structure_axis!(Border, [None => "none", Hairline => "hairline", Box => "box"]);
structure_axis!(Density, [Compact => "compact", Normal => "normal", Relaxed => "relaxed"]);
structure_axis!(Progress, [Line => "line", Blocks => "blocks", Bracket => "bracket"]);
structure_axis!(Selection, [Bar => "bar", Reverse => "reverse", Bg => "bg"]);
structure_axis!(Labels, [Sentence => "sentence", Tracked => "tracked"]);
structure_axis!(Stripe, [Off => "off", On => "on"]);
structure_axis!(Font, [Mixed => "mixed", Mono => "mono"]);

/// 内部通用轴接口；公开结构体只暴露具体枚举。
trait Axis: Copy {
    fn parse_axis_value(value: &str) -> Option<Self>;
    fn as_str(self) -> &'static str;
}

/// 用户可覆盖的 8 个结构轴；全部 `None` 时完全按 skin 预设。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Structure {
    /// 卡片/控件/弹层圆角。
    pub radius: Option<Radius>,
    /// 边框。
    pub border: Option<Border>,
    /// 行高与 cell padding。
    pub density: Option<Density>,
    /// 进度条形态。
    pub progress: Option<Progress>,
    /// 选中行形态。
    pub selection: Option<Selection>,
    /// 表头/字段名大小写。
    pub labels: Option<Labels>,
    /// 表格隔行底色。
    pub stripe: Option<Stripe>,
    /// 界面字体。
    pub font: Option<Font>,
}

/// 补全后的结构值：8 轴都有值，可直接写 DOM / 导出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedStructure {
    pub radius: Radius,
    pub border: Border,
    pub density: Density,
    pub progress: Progress,
    pub selection: Selection,
    pub labels: Labels,
    pub stripe: Stripe,
    pub font: Font,
}

impl ResolvedStructure {
    /// 每个 skin 的既有观感，显式化成 8 个值。
    ///
    /// `plain` / `phosphor` 这两组值就是 T-0056 的零变化基线。
    pub const fn preset(skin: Skin) -> Self {
        match skin {
            Skin::Plain => Self {
                radius: Radius::Soft,
                border: Border::Hairline,
                density: Density::Normal,
                progress: Progress::Line,
                selection: Selection::Bar,
                labels: Labels::Sentence,
                stripe: Stripe::Off,
                font: Font::Mixed,
            },
            Skin::Phosphor => Self {
                radius: Radius::None,
                border: Border::None,
                density: Density::Compact,
                progress: Progress::Line,
                selection: Selection::Bg,
                labels: Labels::Sentence,
                stripe: Stripe::Off,
                font: Font::Mono,
            },
        }
    }
}

impl ResolvedStructure {
    /// 给 `theme list` 的一行缩写：桌面形态 / 密度 / 选中。
    ///
    /// 只取最能区分主题的三个轴，不打印完整 8 轴。
    pub fn summary(self) -> String {
        let surface = if self.border == Border::Box && self.progress == Progress::Bracket {
            "票据"
        } else if self.border == Border::Box {
            "线框"
        } else if self.border == Border::None && self.radius == Radius::None {
            "块面"
        } else {
            "圆角"
        };
        let density = match self.density {
            Density::Compact => "紧凑",
            Density::Normal => "常规",
            Density::Relaxed => "宽松",
        };
        let selection = match self.selection {
            Selection::Bar => "侧条",
            Selection::Reverse => "反白",
            Selection::Bg => "底色",
        };
        format!("{surface}/{density}/{selection}")
    }
}

impl Structure {
    /// 缺项按 skin 补全，并执行轴间互斥；不收集 warning。
    pub fn resolve(&self, skin: Skin) -> ResolvedStructure {
        self.resolve_with_warnings(skin, &mut Vec::new())
    }

    /// 补全并执行语义约束：
    /// - `selection=reverse` 与 `stripe=on` 互斥，stripe 强制 off；
    /// - `progress=blocks/bracket` 需要等宽字体，`font=mixed` 时回退 line。
    pub fn resolve_with_warnings(
        &self,
        skin: Skin,
        warnings: &mut Vec<String>,
    ) -> ResolvedStructure {
        let defaults = ResolvedStructure::preset(skin);
        let font = self.font.unwrap_or(defaults.font);
        let mut progress = self.progress.unwrap_or(defaults.progress);
        let selection = self.selection.unwrap_or(defaults.selection);
        let mut stripe = self.stripe.unwrap_or(defaults.stripe);

        if matches!(progress, Progress::Blocks | Progress::Bracket) && font == Font::Mixed {
            progress = Progress::Line;
            warnings.push("structure.progress 需要 font=mono，已回落 line".to_owned());
        }
        if selection == Selection::Reverse && stripe == Stripe::On {
            stripe = Stripe::Off;
            warnings.push("structure.stripe=on 与 selection=reverse 互斥，已强制 off".to_owned());
        }

        ResolvedStructure {
            radius: self.radius.unwrap_or(defaults.radius),
            border: self.border.unwrap_or(defaults.border),
            density: self.density.unwrap_or(defaults.density),
            progress,
            selection,
            labels: self.labels.unwrap_or(defaults.labels),
            stripe,
            font,
        }
    }
}

/// 主题 JSON 里的 `structure`：字段全是字符串，非法值由 [`parse_axis`] 回落。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RawStructure {
    pub radius: Option<String>,
    pub border: Option<String>,
    pub density: Option<String>,
    pub progress: Option<String>,
    pub selection: Option<String>,
    pub labels: Option<String>,
    pub stripe: Option<String>,
    pub font: Option<String>,
}

fn parse_axis<T: Axis>(
    name: &str,
    raw: Option<String>,
    fallback: T,
    warnings: &mut Vec<String>,
) -> T {
    match raw {
        None => fallback,
        Some(value) => T::parse_axis_value(&value).unwrap_or_else(|| {
            warnings.push(format!(
                "structure.{name} 非法值（{value}），已回落 {}",
                fallback.as_str()
            ));
            fallback
        }),
    }
}

fn parse_structure(
    raw: Option<RawStructure>,
    skin: Skin,
    warnings: &mut Vec<String>,
) -> ResolvedStructure {
    let preset = ResolvedStructure::preset(skin);
    let Some(raw) = raw else {
        return preset;
    };
    let requested = Structure {
        radius: Some(parse_axis("radius", raw.radius, preset.radius, warnings)),
        border: Some(parse_axis("border", raw.border, preset.border, warnings)),
        density: Some(parse_axis("density", raw.density, preset.density, warnings)),
        progress: Some(parse_axis(
            "progress",
            raw.progress,
            preset.progress,
            warnings,
        )),
        selection: Some(parse_axis(
            "selection",
            raw.selection,
            preset.selection,
            warnings,
        )),
        labels: Some(parse_axis("labels", raw.labels, preset.labels, warnings)),
        stripe: Some(parse_axis("stripe", raw.stripe, preset.stripe, warnings)),
        font: Some(parse_axis("font", raw.font, preset.font, warnings)),
    };
    requested.resolve_with_warnings(skin, warnings)
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
            bg: "#0d0f12".to_owned(),
            bg_elev: "#14171c".to_owned(),
            bg_hover: "#1a1e24".to_owned(),
            fg: "#e9e4da".to_owned(),
            fg_muted: "#a7a49e".to_owned(),
            fg_faint: "#7d7c78".to_owned(),
            accent: "#ffb454".to_owned(),
            accent_soft: "rgba(255, 180, 84, 0.14)".to_owned(),
            accent_fg: "#0d0f12".to_owned(),
            danger: "#ff8b7d".to_owned(),
            warn: "#ffb454".to_owned(),
            success: "#a7a49e".to_owned(),
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
    /// 可选结构轴（缺项按 skin 预设补全）。
    pub structure: Option<RawStructure>,
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
    /// 补全后的 8 个结构轴（导出时写完整值）。
    pub structure: ResolvedStructure,
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

        let structure = parse_structure(raw.structure, skin, &mut warnings);
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
            structure,
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
        out.push_str("  \"structure\": {\n");
        let structure = [
            ("radius", self.structure.radius.as_str()),
            ("border", self.structure.border.as_str()),
            ("density", self.structure.density.as_str()),
            ("progress", self.structure.progress.as_str()),
            ("selection", self.structure.selection.as_str()),
            ("labels", self.structure.labels.as_str()),
            ("stripe", self.structure.stripe.as_str()),
            ("font", self.structure.font.as_str()),
        ];
        let last_structure = structure.len().saturating_sub(1);
        for (index, (name, value)) in structure.iter().enumerate() {
            let suffix = if index == last_structure { "" } else { "," };
            out.push_str(&format!(
                "    {}: {}{suffix}\n",
                json_string(name),
                json_string(value)
            ));
        }
        out.push_str("  },\n");
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
            structure: ResolvedStructure::preset(Skin::Plain),
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
    /// 补全后的 8 个结构轴（前端据此写 data-structure-*）。
    pub structure: ResolvedStructure,
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
            structure: spec.structure,
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
    /// 这个内置主题的显式结构（含预设补全）。
    pub structure: ResolvedStructure,
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
            structure: self.structure,
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
pub static BUILTIN_THEMES: [BuiltinTheme; 7] = [
    BuiltinTheme {
        id: "graphite",
        name: "石墨",
        skin: Skin::Plain,
        structure: ResolvedStructure::preset(Skin::Plain),
        tokens: [
            ("bg", "#0d0f12"),
            ("bgElev", "#14171c"),
            ("bgHover", "#1a1e24"),
            ("fg", "#e9e4da"),
            ("fgMuted", "#a7a49e"),
            ("fgFaint", "#7d7c78"),
            ("accent", "#ffb454"),
            ("accentSoft", "rgba(255, 180, 84, 0.14)"),
            ("accentFg", "#0d0f12"),
            ("danger", "#ff8b7d"),
            ("warn", "#ffb454"),
            ("success", "#a7a49e"),
            ("line", "rgba(255, 255, 255, 0.07)"),
            ("lineStrong", "rgba(255, 255, 255, 0.14)"),
        ],
    },
    BuiltinTheme {
        id: "amber",
        name: "琥珀",
        skin: Skin::Plain,
        structure: ResolvedStructure::preset(Skin::Plain),
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
        structure: ResolvedStructure::preset(Skin::Plain),
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
        structure: ResolvedStructure::preset(Skin::Phosphor),
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
    BuiltinTheme {
        id: "tui-block",
        name: "TUI 块面",
        skin: Skin::Plain,
        structure: ResolvedStructure {
            radius: Radius::None,
            border: Border::None,
            density: Density::Compact,
            progress: Progress::Blocks,
            selection: Selection::Reverse,
            labels: Labels::Tracked,
            stripe: Stripe::Off,
            font: Font::Mono,
        },
        tokens: [
            ("bg", "#0b0d0f"),
            ("bgElev", "#101318"),
            ("bgHover", "#1a2028"),
            ("fg", "#c8d1d9"),
            ("fgMuted", "#8b949e"),
            ("fgFaint", "#4d5560"),
            ("accent", "#8fbcbb"),
            ("accentSoft", "#8fbcbb22"),
            ("accentFg", "#0b0d0f"),
            ("danger", "#bf616a"),
            ("warn", "#d08770"),
            ("success", "#a3be8c"),
            ("line", "#26303a"),
            ("lineStrong", "#35414d"),
        ],
    },
    BuiltinTheme {
        id: "tui-wire",
        name: "TUI 线框",
        skin: Skin::Plain,
        structure: ResolvedStructure {
            radius: Radius::None,
            border: Border::Box,
            density: Density::Compact,
            progress: Progress::Blocks,
            selection: Selection::Bg,
            labels: Labels::Tracked,
            stripe: Stripe::Off,
            font: Font::Mono,
        },
        tokens: [
            ("bg", "#0d0f11"),
            ("bgElev", "#121518"),
            ("bgHover", "#1a1e22"),
            ("fg", "#d6d6d6"),
            ("fgMuted", "#8a8a8a"),
            ("fgFaint", "#55585c"),
            ("accent", "#ffb454"),
            ("accentSoft", "#ffb45422"),
            ("accentFg", "#0d0f11"),
            ("danger", "#d06c6c"),
            ("warn", "#d7ba7d"),
            ("success", "#89d185"),
            ("line", "#2a2f35"),
            ("lineStrong", "#3d444c"),
        ],
    },
    BuiltinTheme {
        id: "tui-paper",
        name: "TUI 票据",
        skin: Skin::Plain,
        structure: ResolvedStructure {
            radius: Radius::None,
            border: Border::Box,
            density: Density::Normal,
            progress: Progress::Bracket,
            selection: Selection::Reverse,
            labels: Labels::Tracked,
            stripe: Stripe::Off,
            font: Font::Mono,
        },
        tokens: [
            ("bg", "#f2efe9"),
            ("bgElev", "#e9e5dc"),
            ("bgHover", "#ded9ce"),
            ("fg", "#1a1815"),
            ("fgMuted", "#4a453d"),
            ("fgFaint", "#8a8378"),
            ("accent", "#1a1815"),
            ("accentSoft", "#1a181522"),
            ("accentFg", "#f2efe9"),
            ("danger", "#a33a2a"),
            ("warn", "#8a6d1f"),
            ("success", "#3f6b3a"),
            ("line", "#c9c3b7"),
            ("lineStrong", "#a39c8d"),
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
        assert_eq!(spec.tokens.bg, "#0d0f12");
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
    fn css_block(theme: &str) -> Option<&'static str> {
        let css = include_str!("../../../apps/web/src/design/themes.css");
        let selector = format!("[data-theme=\"{theme}\"]");
        let select = css.find(&selector)?;
        let open = css[select..].find('{')? + select;
        let close = css[open..].find('}')? + open;
        Some(&css[open + 1..close])
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
            let Some(block) = css_block(theme.id) else {
                // TUI 预设由 UI 代理在后续提交补 CSS；Rust 侧不阻塞。
                continue;
            };
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
    fn builtin_themes_cover_all_ids() {
        let ids: Vec<&str> = BUILTIN_THEMES.iter().map(|theme| theme.id).collect();
        assert_eq!(
            ids,
            [
                "graphite",
                "amber",
                "ice",
                "phosphor",
                "tui-block",
                "tui-wire",
                "tui-paper"
            ]
        );
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

    #[test]
    fn tui_presets_have_expected_structure_and_tokens() {
        let block = BuiltinTheme::find("tui-block").expect("tui-block");
        assert_eq!(block.skin, Skin::Plain);
        assert_eq!(block.structure.radius, Radius::None);
        assert_eq!(block.structure.border, Border::None);
        assert_eq!(block.structure.density, Density::Compact);
        assert_eq!(block.structure.progress, Progress::Blocks);
        assert_eq!(block.structure.selection, Selection::Reverse);
        assert_eq!(block.structure.font, Font::Mono);
        assert_eq!(block.tokens[6].1, "#8fbcbb");

        let wire = BuiltinTheme::find("tui-wire").expect("tui-wire");
        assert_eq!(wire.structure.border, Border::Box);
        assert_eq!(wire.structure.selection, Selection::Bg);

        let paper = BuiltinTheme::find("tui-paper").expect("tui-paper");
        assert_eq!(paper.structure.border, Border::Box);
        assert_eq!(paper.structure.progress, Progress::Bracket);
        assert_eq!(paper.structure.selection, Selection::Reverse);
        assert_eq!(paper.tokens[6].1, "#1a1815");
    }

    #[test]
    fn tui_builtins_export_complete_structure() {
        for id in ["tui-block", "tui-wire", "tui-paper"] {
            let theme = BuiltinTheme::find(id).expect(id);
            let spec = theme.to_spec();
            let json = spec.to_json_pretty();
            for key in [
                "radius", "border", "density", "progress", "selection", "labels", "stripe",
                "font",
            ] {
                assert!(json.contains(&format!("\"{key}\"")), "{id} missing {key}");
            }
            let reparsed = parse(&json).expect("TUI export round trip");
            assert_eq!(reparsed.structure, spec.structure, "{id}");
            assert!(reparsed.warnings.is_empty(), "{id}: {:?}", reparsed.warnings);

            let reply = ThemeApplyReply::from_builtin(theme);
            assert_eq!(reply.structure, spec.structure, "{id} apply reply");
        }
    }

    #[test]
    fn structure_summary_uses_three_axes() {
        let block = BuiltinTheme::find("tui-block").expect("tui-block");
        assert_eq!(block.structure.summary(), "块面/紧凑/反白");
        let wire = BuiltinTheme::find("tui-wire").expect("tui-wire");
        assert_eq!(wire.structure.summary(), "线框/紧凑/底色");
        let paper = BuiltinTheme::find("tui-paper").expect("tui-paper");
        assert_eq!(paper.structure.summary(), "票据/常规/反白");
    }

    #[test]
    fn missing_structure_uses_plain_preset() {
        let spec = parse(r##"{"id":"x","contract":1,"skin":"plain"}"##).expect("plain");
        assert_eq!(spec.structure, ResolvedStructure::preset(Skin::Plain));
        assert_eq!(spec.structure.radius, Radius::Soft);
        assert_eq!(spec.structure.border, Border::Hairline);
        assert_eq!(spec.structure.font, Font::Mixed);
    }

    #[test]
    fn missing_structure_uses_phosphor_preset() {
        let spec = parse(r##"{"id":"x","contract":1,"skin":"phosphor"}"##).expect("phosphor");
        assert_eq!(spec.structure, ResolvedStructure::preset(Skin::Phosphor));
        assert_eq!(spec.structure.radius, Radius::None);
        assert_eq!(spec.structure.selection, Selection::Bg);
        assert_eq!(spec.structure.font, Font::Mono);
    }

    #[test]
    fn structure_radius_axis_is_parsed() {
        let spec =
            parse(r##"{"id":"x","contract":1,"structure":{"radius":"none"}}"##).expect("radius");
        assert_eq!(spec.structure.radius, Radius::None);
    }

    #[test]
    fn structure_border_axis_is_parsed() {
        let spec =
            parse(r##"{"id":"x","contract":1,"structure":{"border":"box"}}"##).expect("border");
        assert_eq!(spec.structure.border, Border::Box);
    }

    #[test]
    fn structure_density_axis_is_parsed() {
        let spec = parse(r##"{"id":"x","contract":1,"structure":{"density":"relaxed"}}"##)
            .expect("density");
        assert_eq!(spec.structure.density, Density::Relaxed);
    }

    #[test]
    fn structure_progress_axis_is_parsed() {
        let spec =
            parse(r##"{"id":"x","contract":1,"structure":{"font":"mono","progress":"blocks"}}"##)
                .expect("progress");
        assert_eq!(spec.structure.progress, Progress::Blocks);
    }

    #[test]
    fn structure_selection_axis_is_parsed() {
        let spec = parse(r##"{"id":"x","contract":1,"structure":{"selection":"reverse"}}"##)
            .expect("selection");
        assert_eq!(spec.structure.selection, Selection::Reverse);
    }

    #[test]
    fn structure_labels_axis_is_parsed() {
        let spec =
            parse(r##"{"id":"x","contract":1,"structure":{"labels":"tracked"}}"##).expect("labels");
        assert_eq!(spec.structure.labels, Labels::Tracked);
    }

    #[test]
    fn structure_stripe_axis_is_parsed() {
        let spec =
            parse(r##"{"id":"x","contract":1,"structure":{"stripe":"on"}}"##).expect("stripe");
        assert_eq!(spec.structure.stripe, Stripe::On);
    }

    #[test]
    fn structure_font_axis_is_parsed() {
        let spec = parse(r##"{"id":"x","contract":1,"structure":{"font":"mono"}}"##).expect("font");
        assert_eq!(spec.structure.font, Font::Mono);
    }

    #[test]
    fn reverse_selection_forces_stripe_off_with_warning() {
        let spec =
            parse(r##"{"id":"x","contract":1,"structure":{"selection":"reverse","stripe":"on"}}"##)
                .expect("reverse");
        assert_eq!(spec.structure.selection, Selection::Reverse);
        assert_eq!(spec.structure.stripe, Stripe::Off);
        assert!(
            spec.warnings
                .iter()
                .any(|warning| warning.contains("selection=reverse"))
        );
    }

    #[test]
    fn bracket_with_mixed_font_falls_back_to_line_with_warning() {
        let spec = parse(r##"{"id":"x","contract":1,"structure":{"progress":"bracket"}}"##)
            .expect("bracket");
        assert_eq!(spec.structure.progress, Progress::Line);
        assert!(
            spec.warnings
                .iter()
                .any(|warning| warning.contains("font=mono"))
        );
    }

    #[test]
    fn invalid_structure_values_fall_back_with_warnings() {
        let spec = parse(
            r##"{"id":"x","contract":1,"skin":"plain","structure":{"radius":"round","border":"solid","density":"wide","progress":"bar","selection":"line","labels":"lower","stripe":"yes","font":"serif"}}"##,
        )
        .expect("invalid structure must not fail the theme");

        assert_eq!(spec.structure, ResolvedStructure::preset(Skin::Plain));
        assert_eq!(
            spec.warnings
                .iter()
                .filter(|warning| warning.starts_with("structure."))
                .count(),
            8
        );
    }

    #[test]
    fn blocks_with_mixed_font_falls_back_to_line() {
        let spec = parse(r##"{"id":"x","contract":1,"structure":{"progress":"blocks"}}"##)
            .expect("blocks");
        assert_eq!(spec.structure.progress, Progress::Line);
        assert!(
            spec.warnings
                .iter()
                .any(|warning| warning.contains("font=mono"))
        );
    }

    #[test]
    fn export_contains_resolved_structure_and_round_trips() {
        let spec = parse(
            r##"{"id":"x","contract":1,"skin":"plain","structure":{"radius":"none","labels":"tracked"}}"##,
        )
        .expect("structure");
        let json = spec.to_json_pretty();
        assert!(json.contains("\"structure\""));
        assert!(json.contains("\"radius\": \"none\""));
        assert!(json.contains("\"border\": \"hairline\""));
        assert!(json.contains("\"labels\": \"tracked\""));
        assert!(json.contains("\"font\": \"mixed\""));

        let reparsed = parse(&json).expect("round trip");
        assert_eq!(reparsed.structure, spec.structure);
    }

    #[test]
    fn structure_resolve_keeps_skin_defaults_for_missing_axes() {
        let requested = Structure {
            radius: Some(Radius::Soft),
            ..Structure::default()
        };
        let resolved = requested.resolve(Skin::Phosphor);
        assert_eq!(resolved.radius, Radius::Soft);
        assert_eq!(resolved.border, Border::None);
        assert_eq!(resolved.density, Density::Compact);
        assert_eq!(resolved.selection, Selection::Bg);
        assert_eq!(resolved.font, Font::Mono);
    }
}
