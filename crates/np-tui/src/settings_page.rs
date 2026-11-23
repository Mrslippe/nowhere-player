use std::collections::BTreeSet;

use crate::app::App;
use crate::model::{BrowseKind, ViewData, VisualMode, playback_mode_symbol};
use crate::sound::SoundTheme;
use np_app::SettingsHandle;
use np_core::SettingValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsKind {
    /// 分类伪字段：只用于树形展开/折叠，不是真实配置。
    Category,
    Volume,
    PlaybackMode,
    Theme,
    Bool,
    /// 直播缓冲目标：`live.buffer_ms`，100ms 一档。
    BufferMs,
    /// Player 视觉样式：`ui.visual`，七种样式循环。
    VisualMode,
    /// UI 音效主题：`ui.soundTheme`，switch / typewriter / soft。
    SoundTheme,
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

type SettingSpec = (&'static str, &'static str, SettingsKind);
type CategorySpec = (&'static str, &'static [SettingSpec]);

/// 分类顺序固定：播放 / 界面 / 启动 / 音源 / 直播；「其他」只收纳未知键。
const CATEGORIES: [CategorySpec; 5] = [
    (
        "播放",
        &[
            ("playback.volume", "音量", SettingsKind::Volume),
            ("playback.mode", "播放模式", SettingsKind::PlaybackMode),
        ],
    ),
    (
        "界面",
        &[
            ("theme.id", "主题", SettingsKind::Theme),
            ("ui.splash", "启动页", SettingsKind::Bool),
            ("tui.focusMode", "专注模式", SettingsKind::Bool),
            ("ui.motion", "动效", SettingsKind::Bool),
            ("ui.sound", "音效", SettingsKind::Bool),
            ("ui.soundTheme", "音效主题", SettingsKind::SoundTheme),
            ("ui.visual", "视觉样式", SettingsKind::VisualMode),
        ],
    ),
    (
        "启动",
        &[("startup.enabled", "开机自启", SettingsKind::Bool)],
    ),
    (
        "音源",
        &[
            ("source.bilibili.enabled", "B站", SettingsKind::Bool),
            ("source.netease.enabled", "网易云", SettingsKind::Bool),
        ],
    ),
    (
        "直播",
        &[(
            np_core::settings::LIVE_BUFFER_MS_KEY,
            "直播缓冲",
            SettingsKind::BufferMs,
        )],
    ),
];

/// 测试与快照用包装：默认全部展开。
#[cfg(test)]
pub(crate) fn settings_page(settings: &SettingsHandle) -> SettingsPage {
    settings_page_with(settings, &BTreeSet::new(), "MilkGreen")
}

/// 生成可见行；`rows` 与 `fields` 一一对齐，分类行也是伪字段。
pub(crate) fn settings_page_with(
    settings: &SettingsHandle,
    collapsed: &BTreeSet<String>,
    theme_name: &str,
) -> SettingsPage {
    let mut rows = Vec::new();
    let mut fields = Vec::new();
    for (category, entries) in CATEGORIES {
        let key = format!("category:{category}");
        let folded = collapsed.contains(&key);
        rows.push(vec![
            category_row_text(category, entries.len(), folded),
            String::new(),
        ]);
        fields.push(SettingsField {
            key,
            label: category.to_owned(),
            kind: SettingsKind::Category,
            default: None,
        });
        if folded {
            continue;
        }
        for &(field_key, label, kind) in entries {
            rows.push(vec![
                format!("  {label}"),
                settings_value_text(settings, field_key, kind, theme_name),
            ]);
            fields.push(SettingsField {
                key: field_key.to_owned(),
                label: label.to_owned(),
                kind,
                default: Some(default_value(field_key)),
            });
        }
    }
    let known: Vec<&str> = CATEGORIES
        .iter()
        .flat_map(|(_, entries)| entries.iter().map(|(key, _, _)| *key))
        .collect();
    let unknown: Vec<(String, SettingValue)> = settings
        .entries()
        .into_iter()
        .filter(|(key, _)| !known.contains(&key.as_str()))
        .collect();
    if !unknown.is_empty() {
        let key = "category:其他".to_owned();
        let folded = collapsed.contains(&key);
        rows.push(vec![
            category_row_text("其他", unknown.len(), folded),
            String::new(),
        ]);
        fields.push(SettingsField {
            key,
            label: "其他".to_owned(),
            kind: SettingsKind::Category,
            default: None,
        });
        if !folded {
            for (child_key, value) in unknown {
                rows.push(vec![format!("  {child_key}"), value.display()]);
                fields.push(SettingsField {
                    key: child_key,
                    label: String::new(),
                    kind: SettingsKind::ReadOnly,
                    default: None,
                });
            }
        }
    }
    SettingsPage {
        view: ViewData {
            title: "设置".to_owned(),
            columns: vec!["设置".to_owned(), "值".to_owned()],
            rows,
            refs: Vec::new(),
            durations: Vec::new(),
        },
        fields,
    }
}

fn category_row_text(label: &str, count: usize, folded: bool) -> String {
    let marker = if folded { "▸" } else { "▾" };
    format!("{marker} {label} ({count})")
}

/// 重建 Settings 树后按字段 key 找回选中行；找不到时回退并夹取下标。
pub(crate) fn row_for(fields: &[SettingsField], key: Option<&str>, fallback: usize) -> usize {
    key.and_then(|key| fields.iter().position(|field| field.key == key))
        .unwrap_or(fallback)
        .min(fields.len().saturating_sub(1))
}

pub(crate) fn default_value(key: &str) -> SettingValue {
    match key {
        "playback.volume" => SettingValue::Num(1.0),
        "playback.mode" => SettingValue::Str("sequential".to_owned()),
        "theme.id" => SettingValue::Str("milkgreen".to_owned()),
        "ui.splash" | "ui.motion" | "ui.sound" => SettingValue::Bool(true),
        "ui.soundTheme" => SettingValue::Str(SoundTheme::default().id().to_owned()),
        "tui.focusMode" | "startup.enabled" => SettingValue::Bool(false),
        "source.bilibili.enabled" | "source.netease.enabled" => SettingValue::Bool(true),
        "live.buffer_ms" => SettingValue::Num(np_core::settings::LIVE_BUFFER_MS_DEFAULT as f64),
        "ui.visual" => SettingValue::Str(VisualMode::Spectrum.id().to_owned()),
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

fn settings_value_text(
    settings: &SettingsHandle,
    key: &str,
    kind: SettingsKind,
    theme_name: &str,
) -> String {
    match kind {
        SettingsKind::Category => String::new(),
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
            if settings.theme_id().as_deref() == Some(id.as_str()) {
                theme_name.to_owned()
            } else {
                np_core::theme::BuiltinTheme::find(&id)
                    .map(|theme| theme.name.to_owned())
                    .unwrap_or(id)
            }
        }
        SettingsKind::Bool => bool_text(settings.get_bool(key).unwrap_or_else(
            || match default_value(key) {
                SettingValue::Bool(value) => value,
                _ => false,
            },
        )),
        SettingsKind::BufferMs => format!("{} ms", live_buffer_ms(settings)),
        SettingsKind::VisualMode => settings
            .get_str(key)
            .and_then(|id| VisualMode::from_id(&id))
            .unwrap_or_default()
            .label()
            .to_owned(),
        SettingsKind::SoundTheme => settings
            .get_str(key)
            .and_then(|id| SoundTheme::from_id(&id))
            .unwrap_or_default()
            .label()
            .to_owned(),
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
        && row.len() >= 2
    {
        row[1] = text;
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
