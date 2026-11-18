use crate::app::App;
use crate::audio::TuiAudio;
use crate::model::{BrowseKind, ViewData, playback_mode_symbol};
use crate::startup;
use crossterm::event::KeyCode;
use np_app::{SettingsHandle, ThemeApi};
use np_core::SettingValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsKind {
    Volume,
    PlaybackMode,
    Theme,
    Bool,
    /// 直播缓冲目标：`live.buffer_ms`，100ms 一档。
    BufferMs,
    ReadOnly,
}

#[derive(Debug, Clone)]
pub(crate) struct SettingsField {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) kind: SettingsKind,
    pub(crate) default: Option<SettingValue>,
}

/// 编辑态：值只存在 `draft` 里，Enter 才写回设置与播放器。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SettingsEdit {
    pub(crate) index: usize,
    pub(crate) draft: SettingValue,
}

pub(crate) struct SettingsPage {
    pub(crate) view: ViewData,
    pub(crate) fields: Vec<SettingsField>,
}

const FIELDS: [(&str, &str, &str, SettingsKind); 9] = [
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
    (
        "直播",
        np_core::settings::LIVE_BUFFER_MS_KEY,
        "直播缓冲",
        SettingsKind::BufferMs,
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
        "live.buffer_ms" => SettingValue::Num(np_core::settings::LIVE_BUFFER_MS_DEFAULT as f64),
        _ => SettingValue::Bool(false),
    }
}

/// `live.buffer_ms` 的当前值（毫秒）；未设置时用默认 900ms（T-0093 目标）。
///
/// 键由 T-0093 在 `np-core::settings` 提供；这里只做动态读写，不依赖它先落地。
pub(crate) fn live_buffer_ms(settings: &SettingsHandle) -> u64 {
    let default = np_core::settings::LIVE_BUFFER_MS_DEFAULT;
    match settings.get(np_core::settings::LIVE_BUFFER_MS_KEY) {
        Some(SettingValue::Num(value)) if value.is_finite() => (value.round() as i64).clamp(
            np_core::settings::LIVE_BUFFER_MS_MIN as i64,
            np_core::settings::LIVE_BUFFER_MS_MAX as i64,
        ) as u64,
        _ => default,
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
        SettingsKind::BufferMs => format!("{} ms", live_buffer_ms(settings)),
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
    let current = current_value(app, &field, player, settings);
    let value = step_value(app, &field, &current, direction, settings, theme)?;
    set_setting_value(app, &field, value, player, settings, theme)?;
    app.refresh_settings_view(settings);
    Ok(())
}

/// 选择态 Enter：Bool 直接切换提交；其余可编辑项进入编辑态；ReadOnly 不进入。
pub(crate) fn begin_settings_edit(
    app: &mut App,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    if app.settings_edit.is_some() {
        return Ok(());
    }
    let field = selected_field(app)?;
    match field.kind {
        SettingsKind::ReadOnly => {
            app.set_transient_status(format!("{} 是只读配置", field.field_label()));
            Ok(())
        }
        SettingsKind::Bool => adjust_setting(app, 1, player, settings, theme),
        _ => {
            let draft = current_value(app, &field, player, settings);
            let index = app.row_index;
            set_row_value(app, index, draft_row_text(app, &field, &draft));
            app.settings_edit = Some(SettingsEdit { index, draft });
            Ok(())
        }
    }
}

/// 编辑态 ←/→：只改 draft，不写设置、不碰播放器。
pub(crate) fn adjust_settings_edit(
    app: &mut App,
    direction: i32,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    let Some(edit) = app.settings_edit.clone() else {
        return Ok(());
    };
    let field = app
        .settings_fields
        .get(edit.index)
        .cloned()
        .ok_or_else(|| "当前没有可编辑的设置项".to_owned())?;
    if field.kind == SettingsKind::ReadOnly {
        return Err(format!("{} 是只读配置", field.field_label()));
    }
    let draft = step_value(app, &field, &edit.draft, direction, settings, theme)?;
    set_row_value(app, edit.index, draft_row_text(app, &field, &draft));
    if let Some(edit) = app.settings_edit.as_mut() {
        edit.draft = draft;
    }
    Ok(())
}

/// 编辑态 Enter：提交 draft；失败时保留编辑态，由调用方写入错误状态。
pub(crate) fn commit_settings_edit(
    app: &mut App,
    player: &TuiAudio,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<(), String> {
    let Some(edit) = app.settings_edit.clone() else {
        return Ok(());
    };
    let field = app
        .settings_fields
        .get(edit.index)
        .cloned()
        .ok_or_else(|| "当前没有可编辑的设置项".to_owned())?;
    set_setting_value(app, &field, edit.draft, player, settings, theme)?;
    app.settings_edit = None;
    app.refresh_settings_view(settings);
    Ok(())
}

/// 编辑态 Esc：丢弃 draft，恢复显示当前值。
pub(crate) fn cancel_settings_edit(app: &mut App, settings: &SettingsHandle) -> bool {
    if app.settings_edit.take().is_none() {
        return false;
    }
    app.refresh_settings_view(settings);
    app.set_transient_status("已取消修改");
    true
}

/// 编辑态按键：Left/Right 调整，Enter 提交，Esc 取消；其它键一律吞掉。
///
/// 返回 `true` 表示按键已被编辑态消费，事件循环不应再走全局焦点键。
pub(crate) fn handle_settings_edit_key(
    code: KeyCode,
    app: &mut App,
    player: &TuiAudio,
    theme: &dyn ThemeApi,
    settings: &SettingsHandle,
) -> bool {
    if app.settings_edit.is_none() {
        return false;
    }
    match code {
        KeyCode::Left => {
            if let Err(error) = adjust_settings_edit(app, -1, settings, theme) {
                app.set_status(error);
            }
        }
        KeyCode::Right => {
            if let Err(error) = adjust_settings_edit(app, 1, settings, theme) {
                app.set_status(error);
            }
        }
        KeyCode::Enter => {
            if let Err(error) = commit_settings_edit(app, player, settings, theme) {
                app.set_status(error);
            }
        }
        KeyCode::Esc => {
            cancel_settings_edit(app, settings);
        }
        _ => {}
    }
    true
}

fn selected_field(app: &App) -> Result<SettingsField, String> {
    app.settings_fields
        .get(app.row_index)
        .cloned()
        .ok_or_else(|| "当前没有可编辑的设置项".to_owned())
}

fn current_value(
    app: &App,
    field: &SettingsField,
    player: &TuiAudio,
    settings: &SettingsHandle,
) -> SettingValue {
    match field.kind {
        SettingsKind::Volume => SettingValue::Num(f64::from(player.volume().clamp(0.0, 1.0))),
        SettingsKind::PlaybackMode => SettingValue::Str(player.mode_id().to_owned()),
        SettingsKind::Theme => SettingValue::Str(app.theme_id.clone()),
        SettingsKind::Bool => SettingValue::Bool(
            settings
                .get_bool(&field.key)
                .unwrap_or_else(|| default_bool(&field.key)),
        ),
        SettingsKind::BufferMs => SettingValue::Num(live_buffer_ms(settings) as f64),
        SettingsKind::ReadOnly => settings
            .get(&field.key)
            .unwrap_or(SettingValue::Bool(false)),
    }
}

fn step_value(
    app: &App,
    field: &SettingsField,
    current: &SettingValue,
    direction: i32,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<SettingValue, String> {
    let delta = direction.signum();
    match field.kind {
        SettingsKind::Volume => {
            let number = match current {
                SettingValue::Num(value) => *value,
                _ => 1.0,
            };
            // 1% 步进：先用百分比取整，避免浮点误差累积。
            let percent = (number * 100.0).round() as i32 + delta;
            Ok(SettingValue::Num(f64::from(percent.clamp(0, 100)) / 100.0))
        }
        SettingsKind::PlaybackMode => {
            let ids = ["sequential", "list", "single", "shuffle"];
            let current = match current {
                SettingValue::Str(id) => id.as_str(),
                _ => "sequential",
            };
            let index = ids.iter().position(|id| *id == current).unwrap_or(0) as i32;
            let next = ids[(index + delta).rem_euclid(ids.len() as i32) as usize];
            Ok(SettingValue::Str(next.to_owned()))
        }
        SettingsKind::Theme => {
            let entries = theme.list().map_err(|error| error.message)?;
            if entries.is_empty() {
                return Err("没有可用主题".to_owned());
            }
            let fallback = app.theme_id.as_str();
            let current = match current {
                SettingValue::Str(id) => id.as_str(),
                _ => fallback,
            };
            let index = entries
                .iter()
                .position(|entry| entry.id == current)
                .unwrap_or(0) as i32;
            let next = entries[(index + delta).rem_euclid(entries.len() as i32) as usize]
                .id
                .clone();
            Ok(SettingValue::Str(next))
        }
        SettingsKind::Bool => {
            let value = match current {
                SettingValue::Bool(value) => *value,
                _ => settings
                    .get_bool(&field.key)
                    .unwrap_or_else(|| default_bool(&field.key)),
            };
            Ok(SettingValue::Bool(!value))
        }
        SettingsKind::BufferMs => {
            let value = match current {
                SettingValue::Num(value) => *value,
                _ => live_buffer_ms(settings) as f64,
            };
            let next = (value.round() as i64 + i64::from(delta) * 100).clamp(
                np_core::settings::LIVE_BUFFER_MS_MIN as i64,
                np_core::settings::LIVE_BUFFER_MS_MAX as i64,
            );
            Ok(SettingValue::Num(next as f64))
        }
        SettingsKind::ReadOnly => Err("只读配置不能修改".to_owned()),
    }
}

/// 编辑态在值列显示的文本：`‹ 100% ›`。
fn draft_row_text(app: &App, field: &SettingsField, value: &SettingValue) -> String {
    let text = match field.kind {
        SettingsKind::Volume => match value {
            SettingValue::Num(number) => format!("{}%", (number * 100.0).round() as i64),
            _ => "100%".to_owned(),
        },
        SettingsKind::PlaybackMode => match value {
            SettingValue::Str(id) => playback_mode_symbol(id).to_owned(),
            _ => "·".to_owned(),
        },
        SettingsKind::Theme => match value {
            SettingValue::Str(id) if *id == app.theme_id => app.theme_name.clone(),
            SettingValue::Str(id) => id.clone(),
            _ => app.theme_name.clone(),
        },
        SettingsKind::Bool => bool_text(matches!(value, SettingValue::Bool(true))),
        SettingsKind::BufferMs => match value {
            SettingValue::Num(number) => format!("{} ms", number.round() as i64),
            _ => "— ms".to_owned(),
        },
        SettingsKind::ReadOnly => value.display(),
    };
    format!("‹ {text} ›")
}

fn set_row_value(app: &mut App, index: usize, text: String) {
    let Some(view_index) = app
        .view_kind
        .iter()
        .position(|kind| *kind == BrowseKind::Settings)
    else {
        return;
    };
    if let Some(row) = app
        .views
        .get_mut(view_index)
        .and_then(|view| view.rows.get_mut(index))
        && row.len() >= 3
    {
        row[2] = text;
    }
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
    app.set_transient_status(format!("已重置：{}", field.label));
    app.refresh_settings_view(settings);
    Ok(())
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
            app.set_transient_status(format!("音量 {}%", (volume * 100.0).round() as i32));
        }
        SettingsKind::PlaybackMode => {
            let SettingValue::Str(id) = value else {
                return Err("播放模式设置类型错误".to_owned());
            };
            if !player.set_mode_id(&id) {
                return Err(format!("未知播放模式：{id}"));
            }
            save(settings, &field.key, SettingValue::Str(id.clone()))?;
            app.set_transient_status(format!("播放模式 {}", playback_mode_symbol(&id)));
        }
        SettingsKind::Theme => {
            let SettingValue::Str(id) = value else {
                return Err("主题设置类型错误".to_owned());
            };
            app.load_theme(theme, &id);
            let actual = app.theme_id.clone();
            save(settings, &field.key, SettingValue::Str(actual.clone()))?;
            if actual == id {
                app.set_transient_status(format!("主题 {}", app.theme_name));
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
                app.set_transient_status(format!(
                    "{} {} · 下次启动生效",
                    field.label,
                    bool_text(enabled)
                ));
                return Ok(());
            }
            app.set_transient_status(format!("{} {}", field.label, bool_text(enabled)));
        }
        SettingsKind::BufferMs => {
            let SettingValue::Num(number) = value else {
                return Err("直播缓冲设置类型错误".to_owned());
            };
            let milliseconds = (number.round() as i64).clamp(
                np_core::settings::LIVE_BUFFER_MS_MIN as i64,
                np_core::settings::LIVE_BUFFER_MS_MAX as i64,
            );
            save(settings, &field.key, SettingValue::Num(milliseconds as f64))?;
            player.set_live_buffer_target(milliseconds as u64);
            app.set_transient_status(format!("直播缓冲 {milliseconds}ms"));
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
