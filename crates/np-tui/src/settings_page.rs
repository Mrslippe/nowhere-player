use crate::app::App;
use crate::model::{BrowseKind, ViewData, playback_mode_symbol};
use np_app::SettingsHandle;
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

pub(crate) struct SettingsPage {
    pub(crate) view: ViewData,
    pub(crate) fields: Vec<SettingsField>,
}

const FIELDS: [(&str, &str, &str, SettingsKind); 11] = [
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
    // T-0124：追加在末尾，避免既有设置行号与测试整体偏移。
    ("界面", "ui.motion", "动效", SettingsKind::Bool),
    ("界面", "ui.sound", "音效", SettingsKind::Bool),
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

pub(crate) fn default_value(key: &str) -> SettingValue {
    match key {
        "playback.volume" => SettingValue::Num(1.0),
        "playback.mode" => SettingValue::Str("sequential".to_owned()),
        "theme.id" => SettingValue::Str("milkgreen".to_owned()),
        "ui.splash" | "ui.motion" | "ui.sound" => SettingValue::Bool(true),
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

pub(crate) fn bool_text(value: bool) -> String {
    if value { "开" } else { "关" }.to_owned()
}

pub(crate) fn set_row_value(app: &mut App, index: usize, text: String) {
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

impl SettingsField {
    pub(crate) fn field_label(&self) -> &str {
        if self.label.is_empty() {
            &self.key
        } else {
            &self.label
        }
    }
}
