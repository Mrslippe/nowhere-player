//! np-core 主题到终端样式：只实现 TUI 有意义的轴，导出仍保留完整主题。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use np_app::{CmdError, ThemeApi, ThemeExportTarget};
use np_core::theme::{
    BuiltinTheme, RawTheme, THEME_CONTRACT, ThemeError, ThemeSource, ThemeSpec, ThemeTokens,
};
use np_core::theme::{ThemeApplyReply, ThemeListEntry};
use ratatui::style::Color;

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
    pub danger: Color,
    pub warn: Color,
    pub success: Color,
    pub line: Color,
    pub line_strong: Color,
}

impl Palette {
    /// 从 14 个 token 生成；非法颜色保留默认值。
    pub fn from_tokens(tokens: &ThemeTokens) -> (Self, Vec<String>) {
        let default = ThemeTokens::default();
        let mut warnings = Vec::new();
        let bg = Some(color_or(&tokens.bg, &default.bg, &mut warnings));
        let fg = color_or(&tokens.fg, &default.fg, &mut warnings);
        let muted = color_or(&tokens.fg_muted, &default.fg_muted, &mut warnings);
        let faint = color_or(&tokens.fg_faint, &default.fg_faint, &mut warnings);
        let accent = color_or(&tokens.accent, &default.accent, &mut warnings);
        let danger = color_or(&tokens.danger, &default.danger, &mut warnings);
        let warn = color_or(&tokens.warn, &default.warn, &mut warnings);
        let success = color_or(&tokens.success, &default.success, &mut warnings);
        let line = color_or(&tokens.line, &default.line, &mut warnings);
        let line_strong = color_or(&tokens.line_strong, &default.line_strong, &mut warnings);
        (
            Self {
                bg,
                terminal: false,
                fg,
                muted,
                faint,
                accent,
                danger,
                warn,
                success,
                line,
                line_strong,
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
            danger: Color::Red,
            warn: Color::Yellow,
            success: Color::Green,
            line: Color::DarkGray,
            line_strong: Color::DarkGray,
        }
    }
}

fn color_or(value: &str, fallback: &str, warnings: &mut Vec<String>) -> Color {
    parse_color(value)
        .map(|(r, g, b)| Color::Rgb(r, g, b))
        .or_else(|| {
            warnings.push(format!("非法颜色 {value}，回退 {fallback}"));
            parse_color(fallback).map(|(r, g, b)| Color::Rgb(r, g, b))
        })
        .unwrap_or(Color::Rgb(0, 0, 0))
}

fn parse_color(value: &str) -> Option<(u8, u8, u8)> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return match hex.len() {
            3 => Some((
                u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?,
                u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?,
                u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?,
            )),
            6 => Some((
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
            )),
            8 => Some((
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
            )),
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
            let a: f32 = parts[3].parse().ok()?;
            let bg = (13.0, 15.0, 18.0);
            return Some(blend_on((r, g, b), (bg.0 as u8, bg.1 as u8, bg.2 as u8), a));
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
        entries.insert(
            "terminal".to_owned(),
            ThemeListEntry {
                id: "terminal".to_owned(),
                name: "跟随终端".to_owned(),
                skin: np_core::theme::Skin::Plain,
                source: ThemeSource::Builtin,
            },
        );
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
        if id == "terminal" {
            let spec = ThemeSpec {
                id: "terminal".to_owned(),
                name: "跟随终端".to_owned(),
                ..ThemeSpec::default()
            };
            return Ok(ThemeApplyReply::from_spec(spec, ThemeSource::Builtin));
        }
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
                    .with_hint("可用内置主题：graphite / amber / ice / phosphor / tui-block / tui-wire / tui-paper"),
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
    fn terminal_palette_uses_ansi_slots_and_no_background() {
        let palette = Palette::terminal();
        assert!(palette.bg.is_none());
        assert_eq!(palette.fg, Color::Reset);
        assert_eq!(palette.accent, Color::Cyan);
        assert_eq!(palette.danger, Color::Red);
        assert_eq!(palette.warn, Color::Yellow);
        assert_eq!(palette.success, Color::Green);
        matches!(palette.line, Color::DarkGray);
    }

    #[test]
    fn paper_foreground_contrast_is_at_least_seven() {
        let theme = np_core::theme::BuiltinTheme::find("tui-paper").expect("paper");
        let spec = theme.to_spec();
        let fg = parse_color(&spec.tokens.fg).expect("fg");
        let bg = parse_color(&spec.tokens.bg).expect("bg");
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
        let target = api.export("tui-paper").expect("export");
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
        let expected = np_core::theme::BuiltinTheme::find("tui-paper")
            .expect("paper")
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
    fn terminal_is_listed_and_applies_without_tokens() {
        let api = TuiThemeApi::new(std::env::temp_dir().join("np-tui-theme-list"));
        assert!(
            api.list()
                .expect("list")
                .iter()
                .any(|entry| entry.id == "terminal")
        );
        let reply = api.apply("terminal").expect("apply");
        assert_eq!(reply.id, "terminal");
        assert_eq!(reply.name, "跟随终端");
    }
}
