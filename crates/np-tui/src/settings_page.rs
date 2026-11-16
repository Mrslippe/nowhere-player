use crate::app::App;
use crate::audio::TuiAudio;
use crate::model::{ViewData, playback_mode_symbol};
use crate::startup;
use np_app::{SettingsHandle, ThemeApi};
use np_core::SettingValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsKind {
    Volume,
    PlaybackMode,
    Theme,
    Bool,
    ReadOnly,
}

#[derive(Debug, Clone)]
pub(crate) struct SettingsField {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) kind: SettingsKind,
    pub(crate) default: Option<SettingValue>,
}

pub(crate) struct SettingsPage {
    pub(crate) view: ViewData,
    pub(crate) fields: Vec<SettingsField>,
}

const FIELDS: [(&str, &str, &str, SettingsKind); 8] = [
    ("播放", "playback.volume", "音量", SettingsKind::Volume),
    (
        "播放",
        "playback.mode",
        "播放模式",
        SettingsKind::PlaybackMode,
    ),
    ("界面", "theme.id", "主题", SettingsKind::Theme),
    ("界面", "ui.splash", "启动页", SettingsKind::Bool),
    ("界面", "tui.focusMode", "专注模式", SettingsKind::Bool),
    ("启动", "startup.enabled", "开机自启", SettingsKind::Bool),
    ("音源", "source.bilibili.enabled", "B站", SettingsKind::Bool),
    (
        "音源",
        "source.netease.enabled",
        "网易云",
        SettingsKind::Bool,
    ),
];

pub(crate) fn settings_page(settings: &SettingsHandle) -> SettingsPage {
    let mut rows = Vec::new();
    let mut fields = Vec::new();
    for (category, key, label, kind) in FIELDS {
        rows.push(vec![
            category.to_owned(),
            label.to_owned(),
            settings_value_text(settings, key, kind),
        ]);
        fields.push(SettingsField {
            key: key.to_owned(),
            label: label.to_owned(),
            kind,
            default: Some(default_value(key)),
        });
    }
    let known: Vec<&str> = FIELDS.iter().map(|(_, key, _, _)| *key).collect();
    for (key, value) in settings.entries() {
        if known.contains(&key.as_str()) {
            continue;
        }
        rows.push(vec!["其他".to_owned(), key.clone(), value.display()]);
        fields.push(SettingsField {
            key,
            label: String::new(),
            kind: SettingsKind::ReadOnly,
            default: None,
        });
    }
    SettingsPage {
        view: ViewData {
            title: "设置".to_owned(),
            columns: vec!["分类".to_owned(), "设置项".to_owned(), "值".to_owned()],
            rows,
            refs: Vec::new(),
            durations: Vec::new(),
        },
        fields,
    }
}

fn default_value(key: &str) -> SettingValue {
    match key {
        "playback.volume" => SettingValue::Num(1.0),
        "playback.mode" => SettingValue::Str("sequential".to_owned()),
        "theme.id" => SettingValue::Str("milkgreen".to_owned()),
        "ui.splash" => SettingValue::Bool(true),
        "tui.focusMode" | "startup.enabled" => SettingValue::Bool(false),
        "source.bilibili.enabled" | "source.netease.enabled" => SettingValue::Bool(true),
        _ => SettingValue::Bool(false),
    }
}

fn settings_value_text(settings: &SettingsHandle, key: &str, kind: SettingsKind) -> String {
    match kind {
        SettingsKind::Volume => {
            let value = settings
                .get(key)
                .and_then(|value| match value {
                    SettingValue::Num(number) => Some(number),
                    _ => None,
                })
                .unwrap_or(1.0);
            format!("{}%", (value * 100.0).round() as i64)
        }
        SettingsKind::PlaybackMode => {
            let id = settings
                .get_str(key)
                .unwrap_or_else(|| "sequential".to_owned());
            playback_mode_symbol(&id).to_owned()
        }
        SettingsKind::Theme => {
            let id = settings
                .get_str(key)
                .unwrap_or_else(|| "milkgreen".to_owned());
            if id == "milkgreen" {
                "MilkGreen".to_owned()
            } else {
                id
            }
        }
        SettingsKind::Bool => bool_text(settings.get_bool(key).unwrap_or_else(
            || match default_value(key) {
                SettingValue::Bool(value) => value,
                _ => false,
            },
        )),
        SettingsKind::ReadOnly => settings
            .get(key)
            .map(|value| value.display())
            .unwrap_or_else(|| "未设置".to_owned()),
    }
}

fn bool_text(value: bool) -> String {
    if value { "开" } else { "关" }.to_owned()
}

pub(crate) fn adjust_setting(
    app: &mut App,
    direction: i32,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    let field = selected_field(app)?;
    if field.kind == SettingsKind::ReadOnly {
        return Err(format!("{} 是只读配置", field.field_label()));
    }
    let value = next_value(app, &field, direction, player, settings, theme)?;
    set_setting_value(app, &field, value, player, settings, theme)?;
    app.refresh_settings_view(settings);
    Ok(())
}

pub(crate) fn reset_setting(
    app: &mut App,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    let field = selected_field(app)?;
    let Some(value) = field.default.clone() else {
        return Err(format!("{} 是只读配置", field.field_label()));
    };
    set_setting_value(app, &field, value, player, settings, theme)?;
    app.status = format!("已重置：{}", field.label);
    app.refresh_settings_view(settings);
    Ok(())
}

fn selected_field(app: &App) -> Result<SettingsField, String> {
    app.settings_fields
        .get(app.row_index)
        .cloned()
        .ok_or_else(|| "当前没有可编辑的设置项".to_owned())
}

fn next_value(
    app: &App,
    field: &SettingsField,
    direction: i32,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<SettingValue, String> {
    match field.kind {
        SettingsKind::Volume => {
            let step = (player.volume() * 20.0).round() as i32 + direction.signum();
            let next = (step.clamp(0, 20) as f32) / 20.0;
            Ok(SettingValue::Num(f64::from(next)))
        }
        SettingsKind::PlaybackMode => {
            let ids = ["sequential", "list", "single", "shuffle"];
            let current = player.mode_id();
            let index = ids.iter().position(|id| *id == current).unwrap_or(0) as i32;
            let next = ids[(index + direction.signum()).rem_euclid(ids.len() as i32) as usize];
            Ok(SettingValue::Str(next.to_owned()))
        }
        SettingsKind::Theme => {
            let entries = theme.list().map_err(|error| error.message)?;
            if entries.is_empty() {
                return Err("没有可用主题".to_owned());
            }
            let current = app.theme_id.clone();
            let index = entries
                .iter()
                .position(|entry| entry.id == current)
                .unwrap_or(0) as i32;
            let next = entries
                [(index + direction.signum()).rem_euclid(entries.len() as i32) as usize]
                .id
                .clone();
            Ok(SettingValue::Str(next))
        }
        SettingsKind::Bool => {
            let current = settings
                .get_bool(&field.key)
                .unwrap_or_else(|| default_bool(&field.key));
            Ok(SettingValue::Bool(!current))
        }
        SettingsKind::ReadOnly => Err("只读配置不能修改".to_owned()),
    }
}

fn default_bool(key: &str) -> bool {
    match default_value(key) {
        SettingValue::Bool(value) => value,
        _ => false,
    }
}

fn set_setting_value(
    app: &mut App,
    field: &SettingsField,
    value: SettingValue,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    match field.kind {
        SettingsKind::Volume => {
            let SettingValue::Num(number) = value else {
                return Err("音量设置类型错误".to_owned());
            };
            let volume = (number as f32).clamp(0.0, 1.0);
            save(settings, &field.key, SettingValue::Num(f64::from(volume)))?;
            player.set_volume(volume);
            app.status = format!("音量 {}%", (volume * 100.0).round() as i32);
        }
        SettingsKind::PlaybackMode => {
            let SettingValue::Str(id) = value else {
                return Err("播放模式设置类型错误".to_owned());
            };
            if !player.set_mode_id(&id) {
                return Err(format!("未知播放模式：{id}"));
            }
            save(settings, &field.key, SettingValue::Str(id.clone()))?;
            app.status = format!("播放模式 {}", playback_mode_symbol(&id));
        }
        SettingsKind::Theme => {
            let SettingValue::Str(id) = value else {
                return Err("主题设置类型错误".to_owned());
            };
            app.load_theme(theme, &id);
            let actual = app.theme_id.clone();
            save(settings, &field.key, SettingValue::Str(actual.clone()))?;
            if actual == id {
                app.status = format!("主题 {}", app.theme_name);
            }
        }
        SettingsKind::Bool => {
            let SettingValue::Bool(enabled) = value else {
                return Err("开关设置类型错误".to_owned());
            };
            if field.key == "startup.enabled" {
                if enabled {
                    startup::set_enabled(true).map_err(|error| format!("自启设置失败：{error}"))?;
                } else {
                    let _ = startup::set_enabled(false);
                }
            }
            save(settings, &field.key, SettingValue::Bool(enabled))?;
            if field.key == "tui.focusMode" {
                app.status = format!("{} {} · 下次启动生效", field.label, bool_text(enabled));
                return Ok(());
            }
            app.status = format!("{} {}", field.label, bool_text(enabled));
        }
        SettingsKind::ReadOnly => return Err("只读配置不能修改".to_owned()),
    }
    Ok(())
}

fn save(settings: &SettingsHandle, key: &str, value: SettingValue) -> Result<(), String> {
    settings
        .set_and_save(key, value)
        .map_err(|error| error.message)
}

impl SettingsField {
    fn field_label(&self) -> &str {
        if self.label.is_empty() {
            &self.key
        } else {
            &self.label
        }
    }
}
