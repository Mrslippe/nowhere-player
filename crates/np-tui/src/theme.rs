//! np-core 主题到终端样式：只实现 TUI 有意义的轴，导出仍保留完整主题。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use np_app::{CmdError, ThemeApi, ThemeExportTarget};
use np_core::theme::{
    BuiltinTheme, RawTheme, THEME_CONTRACT, ThemeError, ThemeSource, ThemeSpec, ThemeTokens,
};
use np_core::theme::{ThemeApplyReply, ThemeListEntry};
use ratatui::style::{Color, Modifier, Style};

/// 主题色的语义槽位；UI 组件只通过 [`Palette::color`] 取色。
///
/// 同一槽位在不同主题 / 终端回退下会落到不同具体色，组件不能假设槽位之间
/// 存在某种颜色关系（选中态也不再是简单的 `fg` / `bg` 互换）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaletteToken {
    /// 整屏背景；终端回退时是 `Color::Reset`（不填背景）。
    Canvas,
    /// 抬升背景（面板 / 浮层 / 视频控制条）。
    Surface,
    /// 控件 hover 背景。
    SurfaceHover,
    /// 正文前景。
    Text,
    /// 次要前景。
    Muted,
    /// 更弱的前景（占位符 / 分隔）。
    Faint,
    /// 强调色。
    Accent,
    /// 强调色的半透明底（已按当前主题背景合成）。
    ///
    /// 主题契约要求保留该槽位；当前 TUI 还没有半透明底控件消费它。
    #[allow(dead_code)]
    AccentSoft,
    /// 强调色上的前景（按钮文字）。
    AccentOn,
    /// 细线 / 普通边框。
    Border,
    /// 强线 / 粗边框。
    BorderStrong,
    /// 选中行背景（RGB 主题）。
    SelectionBg,
    /// 选中行前景（RGB 主题）。
    SelectionFg,
    /// 禁用控件前景。
    Disabled,
    /// 错误色。
    Danger,
    /// 警告色。
    Warn,
    /// 成功色。
    ///
    /// 主题契约要求保留该槽位；当前 TUI 的成功提示走状态栏 `muted`。
    #[allow(dead_code)]
    Success,
    /// 焦点边框。
    Focus,
}

/// TUI 实际使用的颜色；结构轴只保留导出，不强行映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// `None` = 跟随终端，不填背景。
    pub bg: Option<Color>,
    pub terminal: bool,
    pub fg: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    /// 强调色的半透明底，已按当前主题 `bg` 合成。
    pub accent_soft: Color,
    pub danger: Color,
    pub warn: Color,
    pub success: Color,
    pub line: Color,
    pub line_strong: Color,
    /// 抬升背景（视频控制条 / 标题栏 surface）。
    pub bg_elev: Color,
    /// 控件 hover 背景。
    pub bg_hover: Color,
    /// 强调色上的前景。
    pub accent_fg: Color,
}

impl Palette {
    /// 唯一取色入口：语义 token → 具体颜色。
    pub fn color(&self, token: PaletteToken) -> Color {
        match token {
            PaletteToken::Canvas => self.bg.unwrap_or(Color::Reset),
            PaletteToken::Surface => self.bg_elev,
            PaletteToken::SurfaceHover => self.bg_hover,
            PaletteToken::Text => self.fg,
            PaletteToken::Muted => self.muted,
            PaletteToken::Faint => self.faint,
            PaletteToken::Accent => self.accent,
            PaletteToken::AccentSoft => self.accent_soft,
            PaletteToken::AccentOn => self.accent_fg,
            PaletteToken::Border => self.line,
            PaletteToken::BorderStrong => self.line_strong,
            PaletteToken::SelectionBg => self.fg,
            PaletteToken::SelectionFg => self.bg.unwrap_or(Color::Reset),
            PaletteToken::Disabled => self.faint,
            PaletteToken::Danger => self.danger,
            PaletteToken::Warn => self.warn,
            PaletteToken::Success => self.success,
            PaletteToken::Focus => self.accent,
        }
    }

    /// 整屏背景；`None` = 跟随终端，不铺底色（终端回退主题）。
    pub fn canvas(&self) -> Option<Color> {
        self.bg
    }

    /// 选中 / 光标反白：RGB 主题互换前后景；终端回退没有可互换的具体色，
    /// 交给 `REVERSED` 让终端反转，避免 Reset 对 Reset 不可见。
    pub fn selection_style(&self) -> Style {
        if self.terminal || self.bg.is_none() {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
                .bg(self.color(PaletteToken::SelectionBg))
                .fg(self.color(PaletteToken::SelectionFg))
        }
    }

    /// 从 14 个 token 生成；非法颜色保留默认值。
    ///
    /// `rgba()` / `#RRGGBBAA` 的 alpha 一律按本主题实际 `bg` 合成，不再假设
    /// 固定 graphite 背景；`bg` 自身无法再合成，按不透明色处理。
    pub fn from_tokens(tokens: &ThemeTokens) -> (Self, Vec<String>) {
        let default = ThemeTokens::default();
        let mut warnings = Vec::new();
        let bg_color = color_or(&tokens.bg, &default.bg, &mut warnings, None);
        let bg = Some(bg_color);
        let base = match bg_color {
            Color::Rgb(red, green, blue) => Some((red, green, blue)),
            _ => None,
        };
        let fg = color_or(&tokens.fg, &default.fg, &mut warnings, base);
        let muted = color_or(&tokens.fg_muted, &default.fg_muted, &mut warnings, base);
        let faint = color_or(&tokens.fg_faint, &default.fg_faint, &mut warnings, base);
        let accent = color_or(&tokens.accent, &default.accent, &mut warnings, base);
        let accent_soft = color_or(
            &tokens.accent_soft,
            &default.accent_soft,
            &mut warnings,
            base,
        );
        let danger = color_or(&tokens.danger, &default.danger, &mut warnings, base);
        let warn = color_or(&tokens.warn, &default.warn, &mut warnings, base);
        let success = color_or(&tokens.success, &default.success, &mut warnings, base);
        let line = color_or(&tokens.line, &default.line, &mut warnings, base);
        let line_strong = color_or(
            &tokens.line_strong,
            &default.line_strong,
            &mut warnings,
            base,
        );
        let bg_elev = color_or(&tokens.bg_elev, &default.bg_elev, &mut warnings, base);
        let bg_hover = color_or(&tokens.bg_hover, &default.bg_hover, &mut warnings, base);
        let accent_fg = color_or(&tokens.accent_fg, &default.accent_fg, &mut warnings, base);
        (
            Self {
                bg,
                terminal: false,
                fg,
                muted,
                faint,
                accent,
                accent_soft,
                danger,
                warn,
                success,
                line,
                line_strong,
                bg_elev,
                bg_hover,
                accent_fg,
            },
            warnings,
        )
    }

    pub fn default_dark() -> Self {
        let (palette, _) = Self::from_tokens(&ThemeTokens::default());
        palette
    }

    /// 跟随终端：ANSI 语义槽位，不填背景。
    pub fn terminal() -> Self {
        Self {
            bg: None,
            terminal: true,
            fg: Color::Reset,
            muted: Color::DarkGray,
            faint: Color::DarkGray,
            accent: Color::Cyan,
            // 终端没有 alpha：半透明强调底退化为 hover 灰。
            accent_soft: Color::DarkGray,
            danger: Color::Red,
            warn: Color::Yellow,
            success: Color::Green,
            line: Color::DarkGray,
            line_strong: Color::DarkGray,
            // 终端没有 `#rrggbb`：视频窗口转换时按槽位回退 `default_dark()`。
            bg_elev: Color::Reset,
            bg_hover: Color::DarkGray,
            accent_fg: Color::Reset,
        }
    }
}

/// 视频窗口主题：9 个具体 RGB，顺序与 `theme ...` 协议行一致。
///
/// 视频宿主是独立进程，不读主题 JSON，也不持有 `ratatui::Color`；TUI 每次
/// 主题变化都把这一份颜色通过 stdin 发过去。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoTheme {
    pub accent: [u8; 3],
    /// 控制条 / 标题栏背景（`Palette::bg_elev`）。
    pub surface: [u8; 3],
    pub hover: [u8; 3],
    pub text: [u8; 3],
    pub muted: [u8; 3],
    pub faint: [u8; 3],
    pub line: [u8; 3],
    pub danger: [u8; 3],
    pub accent_fg: [u8; 3],
}

impl VideoTheme {
    /// `Palette → VideoTheme`；非 RGB 的终端槽位逐个回退默认暗色。
    ///
    /// 与 UI 组件一样经由 [`Palette::color`] 取色；9 色协议顺序不变。
    pub fn from_palette(palette: &Palette) -> Self {
        let fallback = default_palette();
        let rgb = |token: PaletteToken| rgb_or(palette.color(token), fallback.color(token));
        Self {
            accent: rgb(PaletteToken::Accent),
            surface: rgb(PaletteToken::Surface),
            hover: rgb(PaletteToken::SurfaceHover),
            text: rgb(PaletteToken::Text),
            muted: rgb(PaletteToken::Muted),
            faint: rgb(PaletteToken::Faint),
            line: rgb(PaletteToken::Border),
            danger: rgb(PaletteToken::Danger),
            accent_fg: rgb(PaletteToken::AccentOn),
        }
    }

    /// 默认暗色（graphite）对应的视频主题。
    pub fn default_dark() -> Self {
        Self::from_palette(default_palette())
    }

    /// 九个颜色，顺序固定：accent / surface / hover / text / muted / faint /
    /// line / danger / accentFg。
    pub fn colors(self) -> [[u8; 3]; 9] {
        [
            self.accent,
            self.surface,
            self.hover,
            self.text,
            self.muted,
            self.faint,
            self.line,
            self.danger,
            self.accent_fg,
        ]
    }

    /// 初始握手与运行期更新共用的协议行：`theme #.. #.. ...\n`。
    ///
    /// 只含颜色，不含 URL / Cookie / 标题，可安全写进 np-video 的 stdin。
    pub fn protocol_line(self) -> String {
        let mut line = String::from("theme");
        for [red, green, blue] in self.colors() {
            line.push_str(&format!(" #{red:02x}{green:02x}{blue:02x}"));
        }
        line.push('\n');
        line
    }
}

fn default_palette() -> &'static Palette {
    static DEFAULT: OnceLock<Palette> = OnceLock::new();
    DEFAULT.get_or_init(Palette::default_dark)
}

fn rgb_or(color: Color, fallback: Color) -> [u8; 3] {
    let channel = |color: Color| match color {
        Color::Rgb(red, green, blue) => Some([red, green, blue]),
        _ => None,
    };
    channel(color)
        .or_else(|| channel(fallback))
        .unwrap_or([0, 0, 0])
}

fn color_or(
    value: &str,
    fallback: &str,
    warnings: &mut Vec<String>,
    base: Option<(u8, u8, u8)>,
) -> Color {
    resolve_color(value, base)
        .map(|(r, g, b)| Color::Rgb(r, g, b))
        .or_else(|| {
            warnings.push(format!("非法颜色 {value}，回退 {fallback}"));
            resolve_color(fallback, base).map(|(r, g, b)| Color::Rgb(r, g, b))
        })
        .unwrap_or(Color::Rgb(0, 0, 0))
}

/// 解析后的颜色：RGB + 不透明度；`#RRGGBBAA` 的 alpha 不能再被丢弃。
#[derive(Debug, Clone, Copy, PartialEq)]
struct ParsedColor {
    rgb: (u8, u8, u8),
    alpha: f32,
}

/// 解析 token 并按当前主题背景合成半透明色。
///
/// `base` 是本主题实际 `bg` 的 RGB；`None`（例如 `bg` 自身）时按不透明处理。
fn resolve_color(value: &str, base: Option<(u8, u8, u8)>) -> Option<(u8, u8, u8)> {
    let parsed = parse_color(value)?;
    Some(match base {
        Some(bg) => blend_on(parsed.rgb, bg, parsed.alpha),
        None => parsed.rgb,
    })
}

fn parse_color(value: &str) -> Option<ParsedColor> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        // 先判 ASCII 再切片：非法 UTF-8 边界不能 panic，只回退。
        if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let opaque = |rgb: (u8, u8, u8)| ParsedColor { rgb, alpha: 1.0 };
        return match hex.len() {
            3 => Some(opaque((
                u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?,
                u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?,
                u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?,
            ))),
            6 => Some(opaque((
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
            ))),
            8 => {
                let alpha = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(ParsedColor {
                    rgb: (
                        u8::from_str_radix(&hex[0..2], 16).ok()?,
                        u8::from_str_radix(&hex[2..4], 16).ok()?,
                        u8::from_str_radix(&hex[4..6], 16).ok()?,
                    ),
                    alpha: f32::from(alpha) / 255.0,
                })
            }
            _ => None,
        };
    }
    if let Some(inner) = value
        .strip_prefix("rgba(")
        .and_then(|v| v.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() == 4 {
            let r = parts[0].parse().ok()?;
            let g = parts[1].parse().ok()?;
            let b = parts[2].parse().ok()?;
            let alpha: f32 = parts[3].parse().ok()?;
            if !alpha.is_finite() {
                return None;
            }
            return Some(ParsedColor {
                rgb: (r, g, b),
                alpha: alpha.clamp(0.0, 1.0),
            });
        }
    }
    None
}

fn blend_on(color: (u8, u8, u8), bg: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let alpha = alpha.clamp(0.0, 1.0);
    (
        (color.0 as f32 * alpha + bg.0 as f32 * (1.0 - alpha)).round() as u8,
        (color.1 as f32 * alpha + bg.1 as f32 * (1.0 - alpha)).round() as u8,
        (color.2 as f32 * alpha + bg.2 as f32 * (1.0 - alpha)).round() as u8,
    )
}

/// WCAG 相对亮度 / 对比度，供亮色主题验收。
#[cfg_attr(not(test), allow(dead_code))]
pub fn contrast_ratio(foreground: (u8, u8, u8), background: (u8, u8, u8)) -> f32 {
    let lum = |rgb: (u8, u8, u8)| {
        let channel = |value: u8| {
            let value = value as f32 / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgb.0) + 0.7152 * channel(rgb.1) + 0.0722 * channel(rgb.2)
    };
    let a = lum(foreground);
    let b = lum(background);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// TUI 的主题服务：内置 + `<dir>/<id>.json`，行为与 Tauri 侧保持一致。
pub struct TuiThemeApi {
    dir: PathBuf,
}

impl TuiThemeApi {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn load_user(&self, path: &Path) -> Result<ThemeSpec, ThemeError> {
        let text = fs::read_to_string(path)
            .map_err(|error| ThemeError::new("E_THEME_IO", format!("读取主题文件失败：{error}")))?;
        let raw: RawTheme = serde_json::from_str(&text).map_err(|error| {
            ThemeError::new("E_THEME_JSON", format!("主题 JSON 解析失败：{error}"))
        })?;
        ThemeSpec::from_raw(raw)
    }

    fn user_entries(&self) -> Vec<ThemeListEntry> {
        let mut entries = Vec::new();
        let Ok(read_dir) = fs::read_dir(&self.dir) else {
            return entries;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Ok(spec) = self.load_user(&path) {
                entries.push(ThemeListEntry {
                    id: spec.id,
                    name: spec.name,
                    skin: spec.skin,
                    source: ThemeSource::User,
                });
            }
        }
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        entries
    }
}

fn to_cmd_error(error: ThemeError) -> CmdError {
    let mut cmd = CmdError::new(&error.code, error.message);
    cmd.hint = error.hint;
    cmd
}

impl ThemeApi for TuiThemeApi {
    fn list(&self) -> Result<Vec<ThemeListEntry>, CmdError> {
        let mut entries: BTreeMap<String, ThemeListEntry> = BTreeMap::new();
        for theme in np_core::theme::BUILTIN_THEMES.iter() {
            entries.insert(theme.id.to_owned(), theme.entry());
        }
        for entry in self.user_entries() {
            entries.insert(entry.id.clone(), entry);
        }
        Ok(entries.into_values().collect())
    }

    fn apply(&self, id: &str) -> Result<ThemeApplyReply, CmdError> {
        let id = id.trim();
        np_core::theme::validate_theme_id(id).map_err(to_cmd_error)?;
        let path = self.dir.join(format!("{id}.json"));
        if path.is_file() {
            let spec = self.load_user(&path).map_err(to_cmd_error)?;
            return Ok(ThemeApplyReply::from_spec(spec, ThemeSource::User));
        }
        BuiltinTheme::find(id)
            .map(ThemeApplyReply::from_builtin)
            .ok_or_else(|| {
                to_cmd_error(
                    ThemeError::new("E_THEME_NOT_FOUND", format!("主题不存在：{id}"))
                        .with_hint("用 theme list 查看可用主题"),
                )
            })
    }

    fn export(&self, id: &str) -> Result<ThemeExportTarget, CmdError> {
        let theme = BuiltinTheme::find(id.trim()).ok_or_else(|| {
            to_cmd_error(
                ThemeError::new("E_THEME_NOT_FOUND", format!("没有内置主题：{id}"))
                    .with_hint("可用内置主题：milkgreen"),
            )
        })?;
        fs::create_dir_all(&self.dir).map_err(|error| {
            to_cmd_error(ThemeError::new(
                "E_THEME_IO",
                format!("无法创建主题目录：{error}"),
            ))
        })?;
        let path = self.dir.join(format!("{}.json", theme.id));
        let path_text = path.to_string_lossy().into_owned();
        if path.is_file() {
            return Ok(ThemeExportTarget {
                path: path_text,
                created: false,
            });
        }
        let spec = theme.to_spec();
        debug_assert_eq!(spec.contract, THEME_CONTRACT);
        fs::write(&path, spec.to_json_pretty()).map_err(|error| {
            to_cmd_error(ThemeError::new(
                "E_THEME_IO",
                format!("写入主题文件失败：{error}"),
            ))
        })?;
        Ok(ThemeExportTarget {
            path: path_text,
            created: true,
        })
    }

    fn broadcast(&self, _id: &str) {
        // TUI 在同一进程里直接 reload，不需要跨窗口广播。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milk_green_foreground_contrast_is_at_least_seven() {
        let theme = np_core::theme::BuiltinTheme::find("milkgreen").expect("milkgreen");
        let spec = theme.to_spec();
        let fg = parse_color(&spec.tokens.fg).expect("fg").rgb;
        let bg = parse_color(&spec.tokens.bg).expect("bg").rgb;
        assert!(
            contrast_ratio(fg, bg) >= 7.0,
            "contrast {}",
            contrast_ratio(fg, bg)
        );
    }

    #[test]
    fn export_keeps_all_eight_structure_axes_and_is_byte_stable() {
        let dir = std::env::temp_dir().join(format!("np-tui-theme-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let api = TuiThemeApi::new(dir.clone());
        let target = api.export("milkgreen").expect("export");
        assert!(target.created);
        let text = fs::read_to_string(&target.path).expect("read export");
        for axis in [
            "radius",
            "border",
            "density",
            "progress",
            "selection",
            "labels",
            "stripe",
            "font",
        ] {
            assert!(text.contains(&format!("\"{axis}\"")), "missing {axis}");
        }
        let expected = np_core::theme::BuiltinTheme::find("milkgreen")
            .expect("milkgreen")
            .to_spec()
            .to_json_pretty();
        assert_eq!(text, expected);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_color_is_replaced_and_reported() {
        let raw = RawTheme {
            id: Some("broken".to_owned()),
            name: Some("Broken".to_owned()),
            contract: Some(THEME_CONTRACT),
            skin: None,
            structure: None,
            tokens: Some(
                [("fg".to_owned(), "not-a-color".to_owned())]
                    .into_iter()
                    .collect(),
            ),
        };
        let spec = ThemeSpec::from_raw(raw).expect("spec");
        assert_eq!(spec.tokens.fg, ThemeTokens::default().fg);
        assert!(
            spec.warnings
                .iter()
                .any(|warning| warning.contains("颜色非法"))
        );
    }

    #[test]
    fn video_theme_reads_all_nine_palette_roles() {
        let theme = VideoTheme::from_palette(&Palette::default_dark());
        assert_eq!(theme.accent, [255, 180, 84]);
        assert_eq!(theme.surface, [20, 23, 28]);
        assert_eq!(theme.hover, [26, 30, 36]);
        assert_eq!(theme.text, [233, 228, 218]);
        assert_eq!(theme.muted, [167, 164, 158]);
        assert_eq!(theme.faint, [125, 124, 120]);
        assert_eq!(theme.line, [30, 32, 35]);
        assert_eq!(theme.danger, [255, 139, 125]);
        assert_eq!(theme.accent_fg, [13, 15, 18]);
        assert_eq!(theme, VideoTheme::default_dark());
    }

    #[test]
    fn milk_green_video_theme_is_light_surface_with_dark_text() {
        let spec = np_core::theme::BuiltinTheme::find("milkgreen")
            .expect("milkgreen")
            .to_spec();
        let (palette, warnings) = Palette::from_tokens(&spec.tokens);
        assert!(warnings.is_empty(), "{warnings:?}");
        let theme = VideoTheme::from_palette(&palette);
        assert_eq!(theme.surface, [228, 233, 223]);
        assert_eq!(theme.hover, [217, 224, 212]);
        assert_eq!(theme.text, [30, 42, 34]);
        assert_eq!(theme.muted, [74, 88, 80]);
        assert_eq!(theme.faint, [138, 151, 142]);
        assert_eq!(theme.accent, [63, 125, 91]);
        assert_eq!(theme.line, [195, 204, 191]);
        assert_eq!(theme.danger, [163, 58, 42]);
        assert_eq!(theme.accent_fg, [238, 241, 234]);
    }

    #[test]
    fn terminal_palette_falls_back_to_the_default_dark_rgb() {
        // 终端主题只有 ANSI 槽位；视频窗口必须拿到具体 RGB，不能把 Reset 发过去。
        assert_eq!(
            VideoTheme::from_palette(&Palette::terminal()),
            VideoTheme::default_dark()
        );
    }

    #[test]
    fn video_theme_line_has_nine_hex_colors_in_fixed_order() {
        let line = VideoTheme::default_dark().protocol_line();
        assert_eq!(
            line,
            "theme #ffb454 #14171c #1a1e24 #e9e4da #a7a49e #7d7c78 #1e2023 #ff8b7d #0d0f12\n"
        );
        let fields: Vec<&str> = line.trim_end().split(' ').collect();
        assert_eq!(fields.len(), 10);
        assert_eq!(fields[0], "theme");
        for field in &fields[1..] {
            assert_eq!(field.len(), 7, "{field}");
            assert!(field.starts_with('#'), "{field}");
            assert!(
                field[1..].bytes().all(|byte| byte.is_ascii_hexdigit()),
                "{field}"
            );
        }
        assert!(!line.contains("http") && !line.contains("token="));
    }

    #[test]
    fn rgba_alpha_composites_on_the_actual_theme_background() {
        // 浅色主题曾经按固定 graphite 混色，rgba 线会变成近黑的脏线。
        let tokens = ThemeTokens {
            bg: "#eef1ea".to_owned(),
            line: "rgba(0, 0, 0, 0.5)".to_owned(),
            line_strong: "rgba(255, 255, 255, 0.14)".to_owned(),
            ..ThemeTokens::default()
        };
        let (palette, warnings) = Palette::from_tokens(&tokens);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(palette.line, Color::Rgb(119, 121, 117));
        assert_eq!(palette.line_strong, Color::Rgb(240, 243, 237));
        assert_ne!(
            palette.line,
            Color::Rgb(7, 8, 9),
            "不能再按固定 graphite 合成"
        );
    }

    #[test]
    fn eight_digit_hex_alpha_is_no_longer_dropped() {
        let parsed = parse_color("#11223344").expect("8 位 hex");
        assert_eq!(parsed.rgb, (0x11, 0x22, 0x33));
        assert!((parsed.alpha - 68.0 / 255.0).abs() < f32::EPSILON);

        let tokens = ThemeTokens {
            fg: "#11223344".to_owned(),
            ..ThemeTokens::default()
        };
        let (palette, warnings) = Palette::from_tokens(&tokens);
        assert!(warnings.is_empty(), "{warnings:?}");
        // 合成到默认背景 #0d0f12，而不是直接取 #112233。
        assert_eq!(palette.fg, Color::Rgb(0x0e, 0x14, 0x1b));
        // alpha 为 FF 时保持原色。
        let tokens = ThemeTokens {
            fg: "#112233ff".to_owned(),
            ..ThemeTokens::default()
        };
        let (palette, _) = Palette::from_tokens(&tokens);
        assert_eq!(palette.fg, Color::Rgb(0x11, 0x22, 0x33));
    }

    #[test]
    fn milk_green_soft_accent_is_composited_on_milk_green() {
        let spec = np_core::theme::BuiltinTheme::find("milkgreen")
            .expect("milkgreen")
            .to_spec();
        let (palette, warnings) = Palette::from_tokens(&spec.tokens);
        assert!(warnings.is_empty(), "{warnings:?}");
        // #3f7d5b22 → alpha 34/255，合成到 #eef1ea。
        assert_eq!(palette.accent_soft, Color::Rgb(215, 226, 215));
    }

    #[test]
    fn invalid_token_color_still_falls_back_to_the_default() {
        let tokens = ThemeTokens {
            accent: "not-a-color".to_owned(),
            ..ThemeTokens::default()
        };
        let (palette, warnings) = Palette::from_tokens(&tokens);
        assert_eq!(palette.accent, Color::Rgb(255, 180, 84));
        assert!(
            warnings.iter().any(|warning| warning.contains("非法颜色")),
            "{warnings:?}"
        );
    }

    #[test]
    fn malformed_colors_return_none_instead_of_panicking() {
        // 多字节字符不能让 hex 切片 panic，只按非法值回退。
        assert_eq!(parse_color("#🎵abcd"), None);
        assert_eq!(parse_color("#éa"), None);
        assert_eq!(parse_color("rgba(1, 2, 3, NaN)"), None);
        assert_eq!(parse_color("rgba(1, 2, 300, 0.5)"), None);
        assert_eq!(
            parse_color("rgba(1, 2, 3, 1.5)").map(|c| c.alpha),
            Some(1.0)
        );

        let mut warnings = Vec::new();
        assert_eq!(
            color_or("#🎵abcd", "#ffb454", &mut warnings, None),
            Color::Rgb(255, 180, 84)
        );
        assert!(!warnings.is_empty());
    }

    #[test]
    fn terminal_selection_is_reversed_instead_of_reset_on_reset() {
        let terminal = Palette::terminal();
        let style = terminal.selection_style();
        assert!(style.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(style.fg, None, "不能显式写 Reset 前台");
        assert_eq!(style.bg, None, "不能显式写 Reset 背景");

        let dark = Palette::default_dark();
        let style = dark.selection_style();
        assert!(!style.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(style.bg, Some(dark.color(PaletteToken::SelectionBg)));
        assert_eq!(style.fg, Some(dark.color(PaletteToken::SelectionFg)));
        assert_ne!(style.bg, style.fg, "RGB 主题选中态仍互换前后景");
    }

    #[test]
    fn palette_tokens_cover_all_semantic_slots() {
        let palette = Palette::default_dark();
        assert_eq!(palette.color(PaletteToken::Canvas), Color::Rgb(13, 15, 18));
        assert_eq!(palette.color(PaletteToken::Surface), palette.bg_elev);
        assert_eq!(palette.color(PaletteToken::SurfaceHover), palette.bg_hover);
        assert_eq!(palette.color(PaletteToken::Text), palette.fg);
        assert_eq!(palette.color(PaletteToken::Muted), palette.muted);
        assert_eq!(palette.color(PaletteToken::Faint), palette.faint);
        assert_eq!(palette.color(PaletteToken::Accent), palette.accent);
        assert_eq!(palette.color(PaletteToken::AccentSoft), palette.accent_soft);
        assert_eq!(palette.color(PaletteToken::AccentOn), palette.accent_fg);
        assert_eq!(palette.color(PaletteToken::Border), palette.line);
        assert_eq!(
            palette.color(PaletteToken::BorderStrong),
            palette.line_strong
        );
        assert_eq!(palette.color(PaletteToken::SelectionBg), palette.fg);
        assert_eq!(
            palette.color(PaletteToken::SelectionFg),
            palette.bg.unwrap_or(Color::Reset)
        );
        assert_eq!(palette.color(PaletteToken::Disabled), palette.faint);
        assert_eq!(palette.color(PaletteToken::Danger), palette.danger);
        assert_eq!(palette.color(PaletteToken::Warn), palette.warn);
        assert_eq!(palette.color(PaletteToken::Success), palette.success);
        assert_eq!(palette.color(PaletteToken::Focus), palette.accent);

        // 终端回退：Canvas 不填背景，其余槽位都是 ANSI 语义色。
        let terminal = Palette::terminal();
        assert_eq!(terminal.canvas(), None);
        assert_eq!(terminal.color(PaletteToken::Canvas), Color::Reset);
        assert_eq!(terminal.color(PaletteToken::Accent), Color::Cyan);
    }

    #[test]
    fn video_theme_keeps_nine_roles_when_soft_accent_changes() {
        let tokens = ThemeTokens {
            accent_soft: "#ff0000".to_owned(),
            ..ThemeTokens::default()
        };
        let (palette, warnings) = Palette::from_tokens(&tokens);
        assert!(warnings.is_empty(), "{warnings:?}");
        let theme = VideoTheme::from_palette(&palette);
        assert_eq!(theme, VideoTheme::default_dark());
        let colors = theme.colors();
        assert_eq!(colors.len(), 9);
        assert_eq!(colors[0], [255, 180, 84], "accent 仍在第一格");
        assert_eq!(colors[8], [13, 15, 18], "accentFg 仍在最后一格");
    }
}
